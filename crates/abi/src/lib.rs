//! The C ABI mind2t-vt publishes, matching the subset of `libghostty-vt` the differential
//! harness verifies.
//!
//! This is the project's thesis made executable: a program compiled against
//! `ghostty/vt/*.h` can link `libmind2t-vt.a` instead of `libghostty-vt.a`, because the
//! symbols, the struct layouts and the behaviour all agree.
//!
//! **What is exported is a subset, and saying so is part of the claim.** libghostty-vt
//! exports 179 symbols; this exports the terminal-core readout the corpus actually verifies:
//! terminal lifecycle, grid references, and cell / row / style reads. Kitty graphics, key and
//! mouse encoding, selection, OSC parsing, SIMD and the formatters are NOT here. A consumer
//! using only the verified surface can swap libraries; one using the rest cannot.

pub mod exports;
/// The same ABI under our own name, `mind2t_vt_*`. See the module card.
pub mod native;

/// The published C types. They live in their own crate so `crates/ghostty` can pin them
/// against the real library's `ghostty_type_json()` without linking two libraries that
/// define the same symbols into one binary.
pub use mind2t_vt_abi_types as types;
pub use mind2t_vt_abi_types::*;
