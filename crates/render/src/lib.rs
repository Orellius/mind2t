//! Turning a published frame into pixels.
//!
//! A CPU rasterizer, deliberately. The atlas, the run-to-column mapping and the damage logic
//! are all backend-agnostic; only the final blit is not, and putting the reference backend on
//! the CPU is what lets "renders vim" be an assertion in CI instead of a screenshot somebody
//! looked at once. A GPU backend drops in behind the same `Canvas` boundary.
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

pub mod atlas;
pub mod canvas;
pub mod color;
pub mod font;
pub mod renderer;

pub use atlas::{Atlas, Glyph, GlyphKey};
pub use canvas::Canvas;
pub use color::{Drawn, Palette, Rgba};
pub use font::{CellMetrics, FontError, FontStack, Resolved};
pub use renderer::Renderer;
