//! Turning a published frame into pixels.
//!
//! Two backends behind one `Surface` seam, with the CPU one as the reference. The atlas, the
//! run-to-column mapping and the damage logic are all backend-agnostic; only the four
//! operations behind `Surface` are not, and putting the reference on the CPU is what lets
//! "renders vim" be an assertion in CI instead of a screenshot somebody looked at once.
//!
//! **The GPU backend is bit-identical to the CPU one, not merely close.** It has no oracle --
//! there is no third implementation to ask who is right -- so the blend is specified as
//! integer arithmetic and the shader runs the same expression, which makes agreement a
//! property of the design rather than of luck. `tests/backend.rs` demands byte equality and
//! proves it can fail on a one-unit error.
//!
//! Two things here are load-bearing beyond this slice.
//!
//! **Every column comes from `Run::column_of`.** Adding an index to a run's start compiles,
//! passes today, and draws every Hebrew line backwards the moment slice 5.5 emits a
//! right-to-left run. The renderer never learns which way a run goes; it asks.
//!
//! **The font stack is plural because it has to be.** Measured on this machine: Menlo maps
//! Hebrew to glyph 0, and Arial Hebrew maps 'A' to glyph 0. Neither font can draw a terminal
//! containing both, so fallback is not an enhancement and the atlas keys on (font, glyph).
//!
//! What is deliberately not here yet: shaping. A cluster's codepoints are rasterized
//! individually and drawn at one pen position, which is right for Latin and approximate for
//! combining marks. Real mark attachment is slice 5.5.

mod images;
pub mod atlas;
pub mod canvas;
pub mod color;
pub mod font;
pub mod gpu;
pub mod mosaic;
pub mod present;
pub mod renderer;
pub mod shape;
pub mod surface;

pub use atlas::{Atlas, Glyph, GlyphData, GlyphKey};
pub use canvas::Canvas;
pub use color::{Drawn, Palette, Rgba};
pub use font::{CellMetrics, FontError, FontStack, Resolved};
pub use gpu::{GpuContext, GpuError, GpuSurface};
pub use present::{Blitter, PresentError, WindowTarget};
pub use renderer::Renderer;
pub use shape::{PositionedGlyph, Shaper, needs_shaping};
pub use surface::{Surface, TruncatingSurface};

/// The wgpu this crate is built against, re-exported.
///
/// Not a convenience: `Blitter::blit` and `WindowTarget` take `wgpu::TextureView` and
/// `wgpu::TextureFormat` in their signatures, so a caller that builds its own render target has
/// to use the SAME wgpu, and a caller that declares its own dependency can silently get a second
/// copy of the crate. Two wgpu versions in one tree do not fail to compile against each other -
/// they produce types that are merely unrelated, and the error arrives as a mismatch nobody
/// wrote. Re-exporting is how the version stops being guessable.
pub use wgpu;
