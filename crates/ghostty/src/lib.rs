//! The oracle: a real libghostty-vt terminal, read out as a `ruuah_vt_snapshot::Snapshot`.
//!
//! Ghostty's terminal core is the reference implementation ruuah-vt is measured against. It
//! is consumed through the same published C ABI that RUUAH's Swift app already links,
//! so agreement here is agreement with a shipping terminal, not with a model of one.

mod convert;
pub mod sys;
pub mod render;
pub mod terminal;

pub use render::RenderState;
pub use terminal::{Error, Terminal};

/// The library's own description of every C struct layout, as JSON.
///
/// libghostty-vt reports this so bindings can verify themselves rather than trust a
/// generator. `tests/abi_layout.rs` uses it to pin every offset this crate depends on.
pub fn type_layout_json() -> &'static str {
    let ptr = unsafe { sys::ghostty_type_json() };
    assert!(!ptr.is_null(), "ghostty_type_json returned null");
    unsafe { std::ffi::CStr::from_ptr(ptr) }
        .to_str()
        .expect("ghostty_type_json is documented to return valid UTF-8 JSON")
}
