//! Purpose: run the corpus and print a report a human can act on.
//! Public surface: the `ruuah-vt-difftest` binary.
//! Why this file: the harness has to be readable at a glance or it will not be run. The
//!   report names the exact cell and field of every disagreement, because "the grids
//!   differ" is not a lead.
//! NOT responsible for: any comparison logic (`run.rs`, `ruuah-vt-snapshot`).
//! Test strategy: the logic it drives is covered by `tests/corpus.rs`; this file is
//!   formatting and process exit only.

use std::process::ExitCode;

use ruuah_vt_difftest::case::{DEFAULT_CORPUS, load};
use ruuah_vt_difftest::run::{Outcome, Verdict, run};

/// Differences printed per case before truncating. The total is always reported, so a
/// truncated list never reads as a complete one.
const SHOWN_PER_CASE: usize = 6;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dump = args.iter().any(|a| a == "--dump");
    let path = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| DEFAULT_CORPUS.to_string());

    let cases = match load(&path) {
        Ok(cases) => cases,
        Err(err) => {
            eprintln!("ruuah-vt-difftest: {err}");
            return ExitCode::FAILURE;
        }
    };

    println!("ruuah-vt differential oracle");
    println!("  oracle    libghostty-vt (Ghostty terminal core, C ABI)");
    println!("  candidate ruuah-vt-core");
    println!("  corpus    {path} ({} cases)\n", cases.len());

    let mut matched = 0usize;
    let mut as_expected = 0usize;

    for case in &cases {
        let outcome = match run(case) {
            Ok(outcome) => outcome,
            Err(err) => {
                eprintln!("ruuah-vt-difftest: {err}");
                return ExitCode::FAILURE;
            }
        };

        if outcome.verdict == Verdict::Match {
            matched += 1;
        }
        let ok = outcome.met_expectation(case);
        if ok {
            as_expected += 1;
        }

        report(case, &outcome, ok, dump);
    }

    println!(
        "{} cases: {matched} match, {} diff. {as_expected}/{} met expectation.",
        cases.len(),
        cases.len() - matched,
        cases.len()
    );

    if as_expected == cases.len() {
        println!("\nHarness verdict: WORKING - agreement and disagreement both detected.");
        ExitCode::SUCCESS
    } else {
        println!("\nHarness verdict: UNEXPECTED - a case did not behave as the corpus declares.");
        ExitCode::FAILURE
    }
}

fn report(case: &ruuah_vt_difftest::Case, outcome: &Outcome, ok: bool, dump: bool) {
    let flag = if ok { "ok" } else { "UNEXPECTED" };
    println!(
        "{:<6} {:<26} {}x{}  expected {:<5} {flag}",
        outcome.verdict.to_string(),
        case.name,
        case.cols,
        case.rows,
        case.expect
    );
    if !case.note.is_empty() {
        println!("       {}", case.note);
    }

    if !outcome.differences.is_empty() {
        println!("       {} differences:", outcome.differences.len());
        for difference in outcome.differences.iter().take(SHOWN_PER_CASE) {
            println!("         {difference}");
        }
        if outcome.differences.len() > SHOWN_PER_CASE {
            println!(
                "         ... and {} more (showing {SHOWN_PER_CASE} of {})",
                outcome.differences.len() - SHOWN_PER_CASE,
                outcome.differences.len()
            );
        }
    }

    if dump {
        println!("\n       --- oracle grid ---");
        print_indented(&outcome.oracle.to_string());
        println!("       --- candidate grid ---");
        print_indented(&outcome.candidate.to_string());
    }

    println!();
}

fn print_indented(text: &str) {
    for line in text.lines() {
        println!("       {line}");
    }
}
