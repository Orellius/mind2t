//! Purpose: own a pseudoterminal and a child process, and turn their output into frames.
//! Public surface: `Host`, `Options`, `Geometry`, `SpawnError`.
//! Why this file: this is the only place in the project that performs I/O, which is what
//!   lets the core stay a pure state machine. The `Terminal` itself lives on the pump thread
//!   and is never shared -- the thread boundary is crossed by published frames alone, so
//!   there is nothing to lock and no way for a renderer to observe a half-parsed sequence.
//! NOT responsible for: parsing (`ruuah-vt-core`), the frame handoff (`ruuah-vt-frame`), or
//!   drawing anything.
//! Test strategy: `tests/child.rs` runs real programs on a real pty and asserts the frame a
//!   renderer would draw, including that the pty is a controlling terminal (a child that
//!   cannot open /dev/tty is the failure this would otherwise hide) and that a resize
//!   reaches the child as SIGWINCH.

use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::fs::{Mode, OFlags};
use rustix::io::{Errno, FdFlags, fcntl_setfd};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{Winsize, tcsetwinsize};
use ruuah_vt_core::Terminal;
use ruuah_vt_core::events::Event;
use ruuah_vt_core::graphics::ImageOp;
use std::collections::HashMap;
use ruuah_vt_frame::{FrameReader, Publisher, channel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub cols: u16,
    pub rows: u16,
}

impl Geometry {
    fn winsize(self) -> Winsize {
        Winsize {
            ws_row: self.rows,
            ws_col: self.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub size: Geometry,
    /// Largest geometry the shared frame buffer can carry.
    ///
    /// Allocated once, because growing it means replacing the buffer both threads point at.
    /// A resize past this is reported rather than drawn wrong, and the frame the renderer
    /// already has stays valid. Removing the ceiling is a slice 6 item.
    pub capacity: Geometry,
    pub scrollback: usize,
}

impl Options {
    pub fn new(cols: u16, rows: u16) -> Options {
        Options {
            size: Geometry { cols, rows },
            // Headroom for a window dragged larger without reallocating anything.
            capacity: Geometry {
                cols: cols.max(400),
                rows: rows.max(160),
            },
            scrollback: 10_000,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("opening a pseudoterminal failed: {0}")]
    Pty(#[source] Errno),
    #[error("opening the terminal device failed: {0}")]
    Slave(#[source] Errno),
    #[error("spawning the child failed: {0}")]
    Child(#[source] io::Error),
    #[error("initial size {size:?} exceeds the frame channel's capacity {capacity:?}")]
    Capacity { size: Geometry, capacity: Geometry },
}

#[derive(Debug, thiserror::Error)]
pub enum ResizeError {
    #[error("{requested:?} exceeds the frame channel's capacity {capacity:?}")]
    ExceedsCapacity {
        requested: Geometry,
        capacity: Geometry,
    },
    #[error("resizing the pty failed: {0}")]
    Pty(#[source] Errno),
}

impl Geometry {
    fn fits(self, capacity: Geometry) -> bool {
        self.cols <= capacity.cols && self.rows <= capacity.rows
    }
}

/// A running child on a pseudoterminal, publishing frames as it produces output.
pub struct Host {
    master: Arc<OwnedFd>,
    /// The device path of the pty's user side.
    ///
    /// Kept because a resize has to be reopened onto rather than held open: Darwin refuses
    /// `TIOCSWINSZ` on the master (see `set_winsize`), and keeping a slave fd around instead
    /// would mean the master never reports EOF when the child exits, since one end would
    /// still be open in this process.
    pts: CString,
    child: Child,
    pump: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    /// A geometry the pump has not applied to the terminal yet. Zero means none.
    pending: Arc<AtomicU64>,
    /// The frame channel's fixed geometry ceiling; `resize` refuses anything past it.
    capacity: Geometry,
    /// Host-facing events the pump drained from the core (OSC 52, notifications, BEL).
    /// A mutex, not the seqlock: events must arrive exactly once and in order, which a
    /// lossy frame protocol cannot promise. The pump holds it for a push, microseconds.
    events: Arc<Mutex<VecDeque<Event>>>,
    /// The kitty image store the renderer reads: (width, height, pixels), keyed by
    /// image id. Pixels land here ONCE and are Arc'd so a 60Hz poll clones pointers,
    /// never megabytes.
    images: Arc<Mutex<HashMap<u32, (u32, u32, Arc<Vec<u8>>)>>>,
}

/// Sets a pty's window size, which is what raises SIGWINCH in the child.
///
/// Measured on macOS 2026-07-28, with raw `libc::ioctl` as well as through rustix: Darwin
/// returns `ENOTTY` for `TIOCSWINSZ` on the **master**, and only accepts it on the user side.
/// Reading it back works from either end. Linux accepts both, so this is the portable order
/// rather than a workaround. Opening the slave transiently is safe -- the child holds its own
/// descriptor, so this one is never the last.
fn set_winsize(pts: &CStr, size: Geometry) -> Result<(), Errno> {
    let slave = rustix::fs::open(pts, OFlags::RDWR | OFlags::NOCTTY, Mode::empty())?;
    tcsetwinsize(&slave, size.winsize())
}

impl Host {
    /// Opens a pty, starts `command` on it, and begins publishing frames.
    pub fn spawn(
        mut command: Command,
        options: Options,
    ) -> Result<(Host, FrameReader), SpawnError> {
        // `Options::new` cannot produce this, but the fields are public. Gating here is what
        // lets the pump treat a failed publish as a bug rather than a swallowed possibility.
        if !options.size.fits(options.capacity) {
            return Err(SpawnError::Capacity {
                size: options.size,
                capacity: options.capacity,
            });
        }
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).map_err(SpawnError::Pty)?;
        grantpt(&master).map_err(SpawnError::Pty)?;
        unlockpt(&master).map_err(SpawnError::Pty)?;
        // Without this the child inherits the master and the pty never reports EOF, because
        // one end stays open in a process that is not reading it.
        fcntl_setfd(&master, FdFlags::CLOEXEC).map_err(SpawnError::Pty)?;

        let pts = ptsname(&master, Vec::new()).map_err(SpawnError::Pty)?;

        let slave = rustix::fs::open(pts.as_c_str(), OFlags::RDWR | OFlags::NOCTTY, Mode::empty())
            .map_err(SpawnError::Slave)?;
        // On the slave, never the master -- see `set_winsize`. Before the spawn, so the
        // child's very first `stty size` already sees the right geometry.
        tcsetwinsize(&slave, options.size.winsize()).map_err(SpawnError::Slave)?;
        // Open across the fork so `pre_exec` can reach it, gone by the time the child runs.
        fcntl_setfd(&slave, FdFlags::CLOEXEC).map_err(SpawnError::Slave)?;
        let slave_fd: RawFd = slave.as_raw_fd();

        command
            .stdin(Stdio::from(slave.try_clone().map_err(SpawnError::Child)?))
            .stdout(Stdio::from(slave.try_clone().map_err(SpawnError::Child)?))
            .stderr(Stdio::from(slave.try_clone().map_err(SpawnError::Child)?));

        // SAFETY: `pre_exec` runs between fork and exec, where only async-signal-safe calls
        // are legal. Both calls here are syscall wrappers that allocate nothing and take no
        // locks: `setsid` makes the child a session leader (required before it may acquire a
        // controlling terminal at all), and `ioctl_tiocsctty` makes the pty that terminal --
        // which is what job control, SIGWINCH and /dev/tty all depend on. Errors are turned
        // into an errno without formatting, so no allocation happens on the failure path
        // either.
        unsafe {
            command.pre_exec(move || {
                rustix::process::setsid().map_err(errno_to_io)?;
                let slave = BorrowedFd::borrow_raw(slave_fd);
                rustix::process::ioctl_tiocsctty(slave).map_err(errno_to_io)?;
                Ok(())
            });
        }

        let child = command.spawn().map_err(SpawnError::Child)?;
        // The parent's copy has to go, or the master never sees the child's side close.
        drop(slave);

        let master = Arc::new(master);
        let (writer, reader) = channel(options.capacity.cols, options.capacity.rows);
        let stop = Arc::new(AtomicBool::new(false));
        let pending = Arc::new(AtomicU64::new(0));

        let events: Arc<Mutex<VecDeque<Event>>> = Arc::new(Mutex::new(VecDeque::new()));
        let images: Arc<Mutex<HashMap<u32, (u32, u32, Arc<Vec<u8>>)>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pump = thread::Builder::new()
            .name("ruuah-vt-pty".to_string())
            .spawn({
                let master = Arc::clone(&master);
                let stop = Arc::clone(&stop);
                let pending = Arc::clone(&pending);
                let events = Arc::clone(&events);
                let images = Arc::clone(&images);
                move || {
                    pump(master, Publisher::new(writer), options, stop, pending, events, images)
                }
            })
            .map_err(SpawnError::Child)?;

        Ok((
            Host {
                master,
                pts,
                child,
                pump: Some(pump),
                stop,
                pending,
                capacity: options.capacity,
                events,
                images,
            },
            reader,
        ))
    }

    /// Sends input to the child, as a keyboard would.
    /// The kitty image store the pump maintains; the renderer resolves placements
    /// against it. Cloned Arc -- cheap, shared, read-locked briefly per draw.
    pub fn image_store(&self) -> Arc<Mutex<HashMap<u32, (u32, u32, Arc<Vec<u8>>)>>> {
        Arc::clone(&self.images)
    }

    /// Drains the events the pump has collected (OSC 52, notifications, BEL), oldest
    /// first. Exactly-once: what this returns is gone from the queue.
    pub fn take_events(&self) -> Vec<Event> {
        let mut queue = self.events.lock().expect("event queue");
        queue.drain(..).collect()
    }

    pub fn send(&self, bytes: &[u8]) -> Result<(), Errno> {
        let mut written = 0;
        while written < bytes.len() {
            match rustix::io::write(&*self.master, &bytes[written..]) {
                Ok(0) => return Err(Errno::IO),
                Ok(n) => written += n,
                Err(Errno::INTR) => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// Resizes the pty, which is what raises SIGWINCH in the child.
    ///
    /// The terminal's own geometry follows on the pump thread, since that is where it lives.
    /// A geometry past the channel's capacity is refused BEFORE the pty sees it -- the child
    /// never learns of a size no frame could carry, the renderer's frame stays valid, and the
    /// caller gets the report `Options` promises instead of a permanently frozen display
    /// (finding 16: three publishes failing into `let _ =`).
    pub fn resize(&self, size: Geometry) -> Result<(), ResizeError> {
        if !size.fits(self.capacity) {
            return Err(ResizeError::ExceedsCapacity {
                requested: size,
                capacity: self.capacity,
            });
        }
        set_winsize(&self.pts, size).map_err(ResizeError::Pty)?;
        self.pending.store(
            u64::from(size.cols) | (u64::from(size.rows) << 16) | (1 << 32),
            Ordering::Relaxed,
        );
        Ok(())
    }

    pub fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }

    /// Stops the pump and reaps the child. Idempotent; `Drop` calls it.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn errno_to_io(errno: Errno) -> io::Error {
    io::Error::from_raw_os_error(errno.raw_os_error())
}

/// How long the pump waits for output before looking at its own mailbox.
///
/// A resize with a silent child would otherwise sit unapplied until the child next said
/// something, which for a program parked at a prompt can be forever.
const TICK: Timespec = Timespec {
    tv_sec: 0,
    tv_nsec: 16_000_000,
};

fn pump(
    master: Arc<OwnedFd>,
    mut publisher: Publisher,
    options: Options,
    stop: Arc<AtomicBool>,
    pending: Arc<AtomicU64>,
    events: Arc<Mutex<VecDeque<Event>>>,
    images: Arc<Mutex<HashMap<u32, (u32, u32, Arc<Vec<u8>>)>>>,
) {
    let mut terminal =
        Terminal::with_scrollback(options.size.cols, options.size.rows, options.scrollback);
    let mut buffer = vec![0u8; 64 * 1024];

    // Every geometry the terminal can wear is gated against the channel's capacity at spawn
    // and at resize, so a failed publish is a bug in that gating -- swallowing it here is how
    // finding 16 froze the display permanently. The expect makes the bug loud instead.
    const GATED: &str = "geometry is gated at spawn and resize, so every frame fits the channel";

    // The child may have been given a geometry it has not printed anything for yet; publish
    // once so a renderer attaching immediately has a frame rather than nothing.
    publisher.publish(&mut terminal).expect(GATED);

    while !stop.load(Ordering::Relaxed) {
        let requested = pending.swap(0, Ordering::Relaxed);
        if requested & (1 << 32) != 0 {
            terminal.resize(requested as u16, (requested >> 16) as u16);
            publisher.publish(&mut terminal).expect(GATED);
        }

        let mut fds = [PollFd::new(&*master, PollFlags::IN)];
        match rustix::event::poll(&mut fds, Some(&TICK)) {
            Ok(0) => continue,
            Ok(_) => {}
            Err(Errno::INTR) => continue,
            Err(_) => break,
        }

        match rustix::io::read(&*master, &mut buffer[..]) {
            // A pty reports the child's side closing as EIO on macOS and as a zero-length
            // read elsewhere. Both mean the same thing here.
            Ok(0) | Err(Errno::IO) => break,
            Ok(read) => {
                terminal.write(&buffer[..read]);
                // DSR/DA replies are owed to the CHILD, ordered against its own
                // output; they go straight back down the pty before the frame ships.
                let owed = terminal.take_replies();
                if !owed.is_empty() {
                    let mut rest = &owed[..];
                    while !rest.is_empty() {
                        match rustix::io::write(&*master, rest) {
                            Ok(0) => break,
                            Ok(wrote) => rest = &rest[wrote..],
                            Err(Errno::INTR) | Err(Errno::AGAIN) => continue,
                            Err(_) => break,
                        }
                    }
                }
                let image_ops = terminal.take_image_ops();
                if !image_ops.is_empty() {
                    let mut store = images.lock().expect("image store");
                    for op in image_ops {
                        match op {
                            ImageOp::Add(id, image) => {
                                store.insert(
                                    id,
                                    (image.width, image.height, Arc::new(image.rgba)),
                                );
                            }
                            ImageOp::Remove(id) => {
                                store.remove(&id);
                            }
                            ImageOp::Clear => store.clear(),
                        }
                    }
                }
                let drained = terminal.take_events();
                if !drained.is_empty() {
                    // Bounded by the core's own cap; a reader that never drains cannot
                    // grow this past one core-queue worth per lock.
                    let mut queue = events.lock().expect("event queue");
                    queue.extend(drained);
                    let excess = queue.len().saturating_sub(256);
                    if excess > 0 {
                        queue.drain(..excess);
                    }
                }
                publisher.publish(&mut terminal).expect(GATED);
            }
            Err(Errno::INTR) | Err(Errno::AGAIN) => continue,
            Err(_) => break,
        }
    }
}
