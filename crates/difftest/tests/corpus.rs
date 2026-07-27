//! Purpose: prove the harness detects agreement AND disagreement, on the real corpus.
//! Public surface: none, this is a test.
//! Why this file: a harness that only ever reports DIFF has not been shown to detect
//!   agreement, and one that only ever reports MATCH has not been shown to detect
//!   disagreement. Slice 0 is not passed until both are demonstrated, so both are
//!   asserted here rather than left to a human reading the output.
//! NOT responsible for: the correctness of either terminal. It asserts the harness.
//! Test strategy: run every declared case and compare the verdict to its expectation.

use ruuah_vt_difftest::case::{DEFAULT_CORPUS, Expectation, load};
use ruuah_vt_difftest::run::{Verdict, run};

#[test]
fn every_case_produces_the_verdict_the_corpus_declares() {
    let cases = load(DEFAULT_CORPUS).expect("the shipped corpus must load");

    let mut wrong = Vec::new();
    for case in &cases {
        let outcome = run(case).expect("the oracle must not fail");
        if !outcome.met_expectation(case) {
            wrong.push(format!(
                "{}: expected {}, got {} ({} differences, first: {})",
                case.name,
                case.expect,
                outcome.verdict,
                outcome.differences.len(),
                outcome
                    .differences
                    .first()
                    .map(|d| d.to_string())
                    .unwrap_or_else(|| "none".to_string())
            ));
        }
    }

    assert!(wrong.is_empty(), "cases behaved unexpectedly:\n  {}", wrong.join("\n  "));
}

#[test]
fn the_corpus_exercises_both_directions() {
    // The guard against a harness that is trivially always-right. If every case expects
    // the same verdict, passing them all proves nothing about the other direction.
    let cases = load(DEFAULT_CORPUS).expect("corpus");

    let matches = cases.iter().filter(|c| c.expect == Expectation::Match).count();
    let diffs = cases.iter().filter(|c| c.expect == Expectation::Diff).count();

    assert!(matches >= 2, "corpus must contain agreeing cases, has {matches}");
    assert!(diffs >= 2, "corpus must contain disagreeing cases, has {diffs}");
}

#[test]
fn an_agreeing_case_reports_zero_differences_not_merely_a_match_verdict() {
    let cases = load(DEFAULT_CORPUS).expect("corpus");
    let case = cases
        .iter()
        .find(|c| c.name == "plain-ascii")
        .expect("plain-ascii is the canonical agreeing case");

    let outcome = run(case).expect("oracle");
    assert_eq!(outcome.verdict, Verdict::Match);
    assert!(
        outcome.differences.is_empty(),
        "unexpected: {:?}",
        outcome.differences
    );
    assert_eq!(outcome.oracle, outcome.candidate);
}

#[test]
fn a_disagreeing_case_names_the_exact_cell_and_field() {
    // The whole value of the harness is specificity. If it can only say "these differ",
    // slice 1 gets no signal from it.
    let cases = load(DEFAULT_CORPUS).expect("corpus");
    let case = cases
        .iter()
        .find(|c| c.name == "sgr-bold")
        .expect("sgr-bold is the canonical disagreeing case");

    let outcome = run(case).expect("oracle");
    assert_eq!(outcome.verdict, Verdict::Diff);

    let paths: Vec<&str> = outcome.differences.iter().map(|d| d.path.as_str()).collect();
    assert!(
        paths.iter().any(|p| p.starts_with("cell[0,")),
        "must locate a specific cell, got {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.ends_with(".style")),
        "must name the style field, got {paths:?}"
    );
    assert!(
        outcome.differences.iter().all(|d| d.oracle != d.candidate),
        "a reported difference must actually differ"
    );
}

#[test]
fn both_sides_receive_the_identical_byte_stream() {
    // Guards the one way this harness could silently lie: feeding the two
    // implementations different input would make every verdict meaningless. Running the
    // same case twice must be deterministic, and a one-byte change must be visible.
    let cases = load(DEFAULT_CORPUS).expect("corpus");
    let case = cases.iter().find(|c| c.name == "plain-ascii").unwrap();

    let first = run(case).expect("oracle");
    let second = run(case).expect("oracle");
    assert_eq!(first.oracle, second.oracle, "the oracle must be deterministic");
    assert_eq!(
        first.candidate, second.candidate,
        "the candidate must be deterministic"
    );

    let mut altered = case.clone();
    altered.bytes = format!("{}!", case.bytes);
    let changed = run(&altered).expect("oracle");
    assert_ne!(
        first.oracle, changed.oracle,
        "a changed stream must change the oracle grid, or the stream is not reaching it"
    );
    assert_ne!(
        first.candidate, changed.candidate,
        "a changed stream must change the candidate grid too"
    );
}
