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

/// Counts `[[case]]` headers in the raw file, trusting nothing in the loader.
///
/// The independence is the entire point. `load` enforces the declared count itself, but that
/// check runs *before* it returns, so a loader that drops cases on the way out passes it --
/// measured, not assumed: with the check in place, truncating the returned list to 30 still
/// printed "30/30 met expectation" and exited 0. Re-deriving the count from the bytes is what
/// closes that, because it shares no code with the thing it is checking.
fn cases_in_the_file() -> usize {
    std::fs::read_to_string(DEFAULT_CORPUS)
        .expect("the shipped corpus must be readable")
        .lines()
        .filter(|line| line.trim() == "[[case]]")
        .count()
}

#[test]
fn every_case_in_the_file_reaches_the_runner() {
    let loaded = load(DEFAULT_CORPUS).expect("corpus").len();
    let in_file = cases_in_the_file();

    assert_eq!(
        loaded, in_file,
        "the file declares {in_file} cases but {loaded} reached the runner; \
         a verdict over a subset is not a verdict over the corpus"
    );
    assert!(in_file >= 97, "the corpus must not shrink below its slice-7 size, got {in_file}");
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
    // The whole value of the harness is specificity. If it can only say "these differ", the
    // next slice gets no signal from it.
    //
    // Deliberately not pinned to a named case: this test used to hard-code `sgr-bold`, and
    // slice 1 implementing SGR broke it. Every slice closes some case, so anchoring on one
    // by name guarantees a false failure per slice. Anchor on the property instead.
    let cases = load(DEFAULT_CORPUS).expect("corpus");
    let disagreeing: Vec<&_> = cases
        .iter()
        .filter(|case| case.expect == Expectation::Diff)
        .collect();
    assert!(
        !disagreeing.is_empty(),
        "the corpus must retain at least one disagreeing case, or this proves nothing"
    );

    let mut located_a_cell = false;
    for case in disagreeing {
        let outcome = run(case).expect("oracle");
        assert_eq!(outcome.verdict, Verdict::Diff, "case {}", case.name);
        assert!(
            !outcome.differences.is_empty(),
            "case {} reported Diff with no differences",
            case.name
        );
        for difference in &outcome.differences {
            assert!(
                !difference.path.is_empty(),
                "case {}: every difference must be located",
                case.name
            );
            assert_ne!(
                difference.oracle, difference.candidate,
                "case {}: a reported difference must actually differ",
                case.name
            );
        }
        located_a_cell |= outcome
            .differences
            .iter()
            .any(|difference| difference.path.starts_with("cell["));
    }

    assert!(
        located_a_cell,
        "no difference anywhere named a specific cell coordinate"
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

/// Authoring tool, not a gate: prints the measured `diff_paths` line for every diff case,
/// ready to paste into `cases.toml`. Run it when the oracle moves and a pinned set needs
/// re-measuring:
///
///     cargo test -p ruuah-vt-difftest --test corpus -- --ignored print_measured --no-capture
#[test]
#[ignore = "authoring tool; prints rather than asserts"]
fn print_measured_diff_paths() {
    let cases = load(DEFAULT_CORPUS).expect("corpus");
    for case in &cases {
        if case.expect != Expectation::Diff {
            continue;
        }
        let outcome = run(case).expect("oracle");
        let mut paths: Vec<&str> = outcome
            .differences
            .iter()
            .map(|d| d.path.as_str())
            .collect();
        paths.sort_unstable();
        paths.dedup();
        let list = paths
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(", ");
        println!("# {}\ndiff_paths = [{list}]", case.name);
    }
}
