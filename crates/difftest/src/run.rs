//! Purpose: feed one byte stream to both implementations and compare what comes back.
//! Public surface: `Outcome`, `Verdict`, `RunError`, `run`.
//! Why this file: this is the differential oracle itself. Everything else in the crate is
//!   loading input or printing output. Both sides receive the identical `&[u8]` and the
//!   identical geometry, and neither is given any information about the other.
//! NOT responsible for: deciding whether a verdict is acceptable (that is the case's
//!   declared expectation, checked by the caller) or formatting (`main.rs`).
//! Test strategy: `tests/corpus.rs` runs every case and asserts the declared expectation.

use ruuah_vt_snapshot::{Difference, Snapshot, diff};

use crate::case::Case;

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("case '{case}': the oracle failed: {source}")]
    Oracle {
        case: String,
        #[source]
        source: ruuah_vt_ghostty::Error,
    },
}

/// Wraps an oracle failure with the case that provoked it.
fn oracle_error(case: &Case) -> impl Fn(ruuah_vt_ghostty::Error) -> RunError + '_ {
    move |source| RunError::Oracle {
        case: case.name.clone(),
        source,
    }
}

/// Whether the two implementations agreed on this case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Match,
    Diff,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Verdict::Match => write!(f, "MATCH"),
            Verdict::Diff => write!(f, "DIFF"),
        }
    }
}

/// The full result of one case, including both grids so they can be dumped.
#[derive(Debug)]
pub struct Outcome {
    pub verdict: Verdict,
    pub differences: Vec<Difference>,
    pub oracle: Snapshot,
    pub candidate: Snapshot,
}

impl Outcome {
    /// True when the case behaved as the corpus declared it would.
    pub fn met_expectation(&self, case: &Case) -> bool {
        matches!(
            (self.verdict, case.expect),
            (Verdict::Match, crate::case::Expectation::Match)
                | (Verdict::Diff, crate::case::Expectation::Diff)
        )
    }
}

/// Runs one case through libghostty-vt and through ruuah-vt, and diffs the results.
///
/// The byte stream is written in a single call to each side. Chunking is a separate
/// property and is covered by the oracle's own tests, not here.
///
/// A case may resize between its two streams. The second stream is what makes the reflowed
/// cursor observable at all: a grid comparison cannot see a cursor that never writes.
pub fn run(case: &Case) -> Result<Outcome, RunError> {
    let bytes = case.bytes.as_bytes();

    let mut oracle_terminal =
        ruuah_vt_ghostty::Terminal::with_scrollback(case.cols, case.rows, case.scrollback).map_err(|source| RunError::Oracle {
            case: case.name.clone(),
            source,
        })?;
    oracle_terminal.write(bytes);
    if let Some(resize) = case.resize {
        oracle_terminal
            .resize(resize.cols, resize.rows)
            .map_err(|source| RunError::Oracle {
                case: case.name.clone(),
                source,
            })?;
    }
    // The dirty flags are reset here, before `after`, so damage reports exactly what the
    // second stream changed rather than everything since the terminal was created.
    let mut render = if case.damage {
        let mut state = ruuah_vt_ghostty::RenderState::new().map_err(oracle_error(case))?;
        state.update(&oracle_terminal).map_err(oracle_error(case))?;
        state.clear_dirty().map_err(oracle_error(case))?;
        Some(state)
    } else {
        None
    };

    oracle_terminal.write(case.after.as_bytes());
    let mut oracle = oracle_terminal
        .snapshot()
        .map_err(|source| RunError::Oracle {
            case: case.name.clone(),
            source,
        })?;

    if let Some(state) = render.as_mut() {
        state.update(&oracle_terminal).map_err(oracle_error(case))?;
        oracle.damage = Some(state.damage().map_err(oracle_error(case))?);
    }

    let mut candidate_terminal = ruuah_vt_core::Terminal::with_scrollback(case.cols, case.rows, case.scrollback);
    candidate_terminal.write(bytes);
    if let Some(resize) = case.resize {
        candidate_terminal.resize(resize.cols, resize.rows);
    }
    if case.damage {
        candidate_terminal.clear_damage();
    }
    candidate_terminal.write(case.after.as_bytes());
    let mut candidate = candidate_terminal.snapshot();
    if case.damage {
        candidate.damage = candidate_terminal.damage();
    }

    let differences = diff(&oracle, &candidate);
    let verdict = if differences.is_empty() {
        Verdict::Match
    } else {
        Verdict::Diff
    };

    Ok(Outcome {
        verdict,
        differences,
        oracle,
        candidate,
    })
}
