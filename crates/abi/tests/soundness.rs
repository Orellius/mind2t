//! Purpose: prove the read entry points are actually reads, in the Rust-model sense.
//! Public surface: none, this is a test.
//! Why this file: the oracle's `ghostty_terminal_get` reads fields off the terminal and
//!   mutates nothing (`terminal.zig`, `getTyped`), and its grid refs die on the next
//!   *update*, not on the next read (`grid_ref.h`). A C consumer is therefore free to read
//!   from two threads at once, or to hold a ref across an interleaved read. The audit's
//!   findings 14 and 22 were both this port breaking that freedom: every read conjured
//!   `&mut Terminal` to lazily refresh a cached snapshot, so two concurrent readers were a
//!   data race and a ref held across a read was an aliasing violation.
//! NOT responsible for: what the reads return -- `differential.rs` owns that.
//! Test strategy: neither defect can fail deterministically in a native run; undefined
//!   behaviour usually looks like passing. The oracle that can say NO is Miri:
//!
//!       cargo +nightly miri test -p ruuah-vt-abi --test soundness
//!
//!   Both tests were run under Miri against the pre-fix code and SEEN to fail there
//!   (a data race on the cached view, and a Stacked Borrows violation through the ref's
//!   node pointer). Natively they still assert the behavioural half: the reads succeed
//!   and agree.

use std::ffi::c_void;

use ruuah_vt::exports::*;
use ruuah_vt::types::*;

unsafe fn new_terminal(cols: u16, rows: u16, bytes: &[u8]) -> GhosttyTerminal {
    unsafe {
        let mut handle: GhosttyTerminal = std::ptr::null_mut();
        assert_eq!(
            ghostty_terminal_new(
                std::ptr::null(),
                &mut handle,
                GhosttyTerminalOptions {
                    cols,
                    rows,
                    max_scrollback: 0,
                },
            ),
            GHOSTTY_SUCCESS
        );
        ghostty_terminal_vt_write(handle, bytes.as_ptr(), bytes.len());
        handle
    }
}

unsafe fn grid_ref_at(handle: GhosttyTerminal, x: u16, y: u32) -> GhosttyGridRef {
    let mut out = GhosttyGridRef {
        size: size_of::<GhosttyGridRef>(),
        node: std::ptr::null_mut(),
        x: 0,
        y: 0,
    };
    let point = GhosttyPoint {
        tag: GHOSTTY_POINT_TAG_ACTIVE,
        value: GhosttyPointValue {
            coordinate: GhosttyPointCoordinate { x, y },
        },
    };
    assert_eq!(
        unsafe { ghostty_terminal_grid_ref(handle, point, &mut out) },
        GHOSTTY_SUCCESS
    );
    out
}

unsafe fn cursor_x(handle: GhosttyTerminal) -> u16 {
    let mut out: u16 = 0;
    assert_eq!(
        unsafe {
            ghostty_terminal_get(
                handle,
                GHOSTTY_TERMINAL_DATA_CURSOR_X,
                (&raw mut out).cast::<c_void>(),
            )
        },
        GHOSTTY_SUCCESS
    );
    out
}

unsafe fn codepoint_through(cell_ref: &GhosttyGridRef) -> u32 {
    let mut raw: GhosttyCell = 0;
    assert_eq!(
        unsafe { ghostty_grid_ref_cell(cell_ref, &mut raw) },
        GHOSTTY_SUCCESS
    );
    let mut cp: u32 = 0;
    assert_eq!(
        unsafe { ghostty_cell_get(raw, GHOSTTY_CELL_DATA_CODEPOINT, (&raw mut cp).cast()) },
        GHOSTTY_SUCCESS
    );
    cp
}

/// A ref held across an interleaved pure read must stay usable. Upstream kills a ref on the
/// next *update to the terminal instance* (`grid_ref.h:37`); `terminal_get` updates nothing,
/// so this sequence is legal C against the oracle. Pre-fix, Miri rejected it: the ref's node
/// pointer was derived from a `&mut Terminal` that `terminal_get` then invalidated by
/// conjuring its own.
#[test]
fn a_grid_ref_survives_an_interleaved_pure_read() {
    unsafe {
        let handle = new_terminal(8, 2, b"hi");

        let cell_ref = grid_ref_at(handle, 0, 0);
        assert_eq!(cursor_x(handle), 2, "the write landed before the ref");
        assert_eq!(
            codepoint_through(&cell_ref),
            u32::from('h'),
            "the ref minted before the read must still resolve after it"
        );

        ghostty_terminal_free(handle);
    }
}

/// The handle crosses threads as a pointer, keeping its provenance; an integer cast would
/// hide exactly what Miri needs to see.
struct SendHandle(GhosttyTerminal);
unsafe impl Send for SendHandle {}

/// Two threads reading the same terminal with no writer in flight -- the renderer-plus-query
/// pattern the oracle permits, since its reads mutate nothing. Pre-fix, both readers wrote
/// the lazily-filled cache and Miri reported the data race. The cache is cold on entry, which
/// is the hostile case: both threads reach the fill.
#[test]
fn concurrent_pure_reads_do_not_race() {
    unsafe {
        let handle = new_terminal(8, 2, b"hi");

        let reader = |handle: &SendHandle| {
            let handle = handle.0;
            unsafe {
                assert_eq!(cursor_x(handle), 2);
                let cell_ref = grid_ref_at(handle, 1, 0);
                assert_eq!(codepoint_through(&cell_ref), u32::from('i'));
            }
        };

        let a = SendHandle(handle);
        let b = SendHandle(handle);
        std::thread::scope(|scope| {
            scope.spawn(move || reader(&a));
            scope.spawn(move || reader(&b));
        });

        ghostty_terminal_free(handle);
    }
}
