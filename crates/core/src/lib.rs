//! The vtr terminal core.
//!
//! Slice 0 has no terminal implementation. What lives here is a stub whose only job is
//! to be the second input to the differential harness, so the harness can be shown to
//! detect both agreement and disagreement before any real work is built on top of it.
//!
//! It is not a design sketch and nothing here should be grown into one. Slice 1 replaces
//! it with the `vte` parser driving a real cell grid.

pub mod terminal;

pub use terminal::Terminal;
