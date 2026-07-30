//! The pty host: the one crate in this project that does I/O.
//!
//! Everything below it is pure. `ruuah-vt-core` is a state machine with no clock and no file
//! descriptors, which is what makes the differential corpus possible at all; that property
//! is only worth anything if nothing quietly adds I/O to it later. So the pty lives here
//! instead, owns the `Terminal` on its own thread, and hands the outside world published
//! frames rather than a shared terminal.
//!
//! Built on `rustix` rather than a pty crate: spawning a pty is three syscalls and a
//! controlling-terminal dance, and the alternative pulled in thirteen crates on macOS
//! including a serial-port library. The one `unsafe` block is the `pre_exec` hook, and it
//! is justified where it sits.

pub mod host;
pub mod mouse;
pub mod paste;

pub use host::{Geometry, Host, Options, ResizeError, SpawnError};
