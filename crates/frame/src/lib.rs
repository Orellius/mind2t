//! Publishing frames from the parse thread to a renderer.
//!
//! The core is a pure state machine and stays that way; this crate is the one-way valve
//! between it and a thread that draws. Two decisions shape everything here.
//!
//! **A seqlock, not a mutex.** The writer never waits. A reader that arrives mid-publish is
//! told so and discards that frame rather than drawing half of one. Ghostty reached for a
//! mutex here and then had to bolt a demand-and-handoff protocol onto it, because under
//! sustained pty output an unfair mutex lets the parse loop relock before a sleeping
//! renderer can be scheduled (`../ruuah/src/renderer/State.zig`). The trade is the mirror
//! image: no fairness problem, but a busy writer can make a reader skip, so the caller
//! decides when to come back.
//!
//! **The renderer sees runs, not cells.** `Frame::runs` yields spans that carry a starting
//! column and a `Direction`, and a renderer asks the run where each cell goes. Slice 5.5
//! turned right-to-left runs on and the renderer was not touched: reordering changed `runs`
//! and `bidi.rs` and nothing else, which is what the seam was for.
//!
//! No `unsafe`, no volatile reads. The shared payload is `AtomicU64` accessed `Relaxed`,
//! which is defined under concurrent access; the generation counter's `Acquire`/`Release`
//! pair is what makes a set of atomic words into a single consistent frame.

pub mod bidi;
pub mod frame;
pub mod packed;
pub mod publish;
pub mod seqlock;

pub use bidi::{BaseDirection, VisualSpan, visual_spans};
pub use frame::{Direction, Frame, FrameCursor, Motion, Run, cell_width};
pub use packed::{CLUSTER_BYTES, PackedCell, pack_style, unpack_style};
pub use publish::Publisher;
pub use seqlock::{CapacityExceeded, FrameReader, FrameWriter, Publish, ReadOutcome, channel};
