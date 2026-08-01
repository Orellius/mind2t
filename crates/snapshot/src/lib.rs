//! The observable state of a terminal, and the difference between two of them.
//!
//! This crate is the contract of the differential oracle: `ruuah-vt-ghostty` and
//! `ruuah-vt-core` both produce a [`Snapshot`], and [`diff`] is the only thing that
//! decides whether they agree. It holds no terminal logic and depends on
//! nothing, so neither implementation can bias the comparison.

pub mod difference;
pub mod grid;

pub use difference::{Difference, diff};
pub use grid::{
    Cell, Color, Colors, Cursor, Damage, Dirty, Modes, Rgb, Row, RowSemantic, Screen, Semantic,
    Snapshot, Style, Underline, Wide, default_palette,
};
