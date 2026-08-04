//! Purpose: the pieces both Bindary binaries share.
//! Public surface: `clipboard`, `keys`, `wheel`.
//! Why this file: there are two hosts - the Tauri app (`main.rs`) and the tao + wry oracle
//!   (`bin/probe.rs`) - and Cargo gives binaries no way to share a module without a library
//!   target. Everything here is host-agnostic by construction; anything that knows which
//!   windowing stack it is inside belongs in that host, not here.
//! NOT responsible for: windows, event loops, or the session. Those differ between the two
//!   hosts, and that difference is the whole point of keeping both.

pub mod clipboard;
pub mod keys;
pub mod wheel;
