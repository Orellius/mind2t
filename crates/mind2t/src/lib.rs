//! Purpose: the pieces both Mind2t binaries share.
//! Public surface: `canvas`, `keys`, `layout`, plus re-exports of `clipboard`, `scrollback`
//!   and `wheel` from `mind2t-vt-host`, which is where those three now live.
//! Why this file: there are two hosts - the Tauri app (`main.rs`) and the tao + wry oracle
//!   (`bin/probe.rs`) - and Cargo gives binaries no way to share a module without a library
//!   target. Everything here is host-agnostic by construction; anything that knows which
//!   windowing stack it is inside belongs in that host, not here.
//! NOT responsible for: windows, event loops, or the session. Those differ between the two
//!   hosts, and that difference is the whole point of keeping both.

pub mod agent;
pub mod canvas;
pub mod keys;
pub mod launch;
pub mod layout;

// Re-exports, not modules: these three moved into `mind2t-vt-host` on 2026-08-11 so the Swift
// host can reach them over the C surface instead of owning a second copy. They are re-exported
// rather than merely moved so that `mind2t::wheel::...` keeps resolving at every existing call
// site and in `tests/`, which makes the move reviewable as a move. When the Tauri host goes,
// these three lines go with it and the call sites read `mind2t_vt_host::` directly.
//
// `keys` did NOT move and cannot: it imports `tao` and translates a `TaoKeyEvent`. It is the
// Tauri key SOURCE, and its Swift counterpart is `KeyMap.swift`, generated from the same table.
pub use mind2t_vt_host::{clipboard, scrollback, wheel};
