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
    Cell, Color, Cursor, Damage, Dirty, Row, Screen, Snapshot, Style, Underline, Wide,
};
