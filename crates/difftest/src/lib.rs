//! The differential oracle harness: one byte stream, two terminals, one verdict.
//!
//! Slice 0 of vtr is a gate on this crate working, not on any terminal being written.
//! If an identical stream can be pushed through libghostty-vt and through vtr and the
//! resulting grids compared precisely, then every slice after this one has a correctness
//! signal. If it cannot, the project has no way to know it is right and should stop.

pub mod case;
pub mod run;

pub use case::{Case, Expectation, load};
pub use run::{Outcome, Verdict, run};
