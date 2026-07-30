//! The second oracle: esctest2 (Dickey's maintained fork of iTerm2's esctest), driven
//! through this crate's own pty host with the child none the wiser.
//!
//! The differential corpus can only see what libghostty-vt's ABI exposes; esctest2 is a
//! vendor-neutral conformance suite that inspects the terminal from the INSIDE -- CPR for
//! the cursor, DECRQCRA checksums for screen content, WINOPS 18 for geometry -- so it
//! gates reply semantics the ABI oracle structurally cannot. It is also the tenth
//! blind-spot lesson applied to replies: before this file, nothing outside our own unit
//! tests had an opinion about them.
//!
//! The pinning law is the corpus's: `corpus/esctest-expected-pass.txt` lists every test
//! asserted to pass TODAY within `GATE_INCLUDE`. A pinned test failing is a regression; an
//! unpinned test passing must be promoted into the file. Both directions are enforced, so
//! the pin can be wrong in either and the gate says so.
//!
//! Suite updates re-run `scripts/fetch-esctest.sh` (the pin lives in `esctest.lock`), then
//! re-triage with the ignored authoring test at the bottom.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use ruuah_vt_pty::{Host, Options};

/// The test universe the gate runs: the whole suite. Measured 2026-07-30 at ~154s wall
/// clock for 568 tests (the 1s reply timeout is rare because the queries esctest leans
/// on -- CPR, DECRQCRA, WINOPS 18 -- are all answered), which buys full both-direction
/// pinning with zero curation drift.
const GATE_INCLUDE: &str = ".";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Passed,
    Failed,
    /// esctest's own "known bug" and insufficient-VT-level outcomes: not our verdicts.
    Skipped,
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/pty sits two levels under the workspace root")
        .to_path_buf()
}

/// Runs esctest2 as the child of a fresh pty host and parses its logfile.
fn run_esctest(include: &str, patience: Duration) -> BTreeMap<String, Outcome> {
    let root = workspace_root();
    let suite = root.join("vendor/esctest2/esctest/esctest.py");
    assert!(
        suite.exists(),
        "esctest2 is not vendored; run ./scripts/fetch-esctest.sh first"
    );

    let log = std::env::temp_dir().join(format!(
        "ruuah-esctest-{}-{include_len}.log",
        std::process::id(),
        include_len = include.len()
    ));
    let _ = fs::remove_file(&log);

    let mut command = Command::new("python3");
    command
        .arg(&suite)
        .arg("--expected-terminal=xterm")
        .arg("--max-vt-level=4")
        .arg("--timeout=1")
        .arg("--no-print-logs")
        .arg(format!("--logfile={}", log.display()))
        .arg(format!("--include={include}"));
    command.env("TERM", "xterm");

    // 80x25, because reset() asks for exactly that via a WINOPS resize -- an op this
    // terminal refuses on purpose (a child resizing the operator's window is not a
    // feature), so the pty must be born at the size the suite assumes.
    let mut options = Options::new(80, 25);
    options.reports = true;
    let (mut host, _reader) = Host::spawn(command, options).expect("spawn esctest");

    let deadline = Instant::now() + patience;
    loop {
        match host.try_wait() {
            Ok(Some(_)) => break,
            _ if Instant::now() > deadline => {
                let so_far = fs::read_to_string(&log).unwrap_or_default();
                panic!(
                    "esctest did not finish within {patience:?}; log tail:\n{}",
                    so_far.lines().rev().take(20).collect::<Vec<_>>().join("\n")
                );
            }
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
    }

    let text = fs::read_to_string(&log).unwrap_or_default();
    let _ = fs::remove_file(&log);
    let results = parse(&text);
    assert!(
        !results.is_empty(),
        "esctest produced no results at all -- the log was:\n{text}"
    );
    results
}

/// The log grammar, from esctest.py's RunTest: "Run test: NAME", then one of
/// "Passed." / "Fails as expected: ..." / "Skipped because ..." / "*** TEST NAME FAILED:".
fn parse(log: &str) -> BTreeMap<String, Outcome> {
    let mut results = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in log.lines() {
        if let Some(name) = line.strip_prefix("Run test: ") {
            current = Some(name.trim().to_string());
            continue;
        }
        let Some(name) = current.clone() else { continue };
        let outcome = if line.starts_with("Passed.") {
            Some(Outcome::Passed)
        } else if line.starts_with("Fails as expected") || line.starts_with("Skipped because") {
            Some(Outcome::Skipped)
        } else if line.starts_with("*** TEST ") && line.contains("FAILED") {
            Some(Outcome::Failed)
        } else {
            None
        };
        if let Some(outcome) = outcome {
            results.insert(name, outcome);
            current = None;
        }
    }
    results
}

fn expected_passes() -> Vec<String> {
    let path = workspace_root().join("corpus/esctest-expected-pass.txt");
    let text = fs::read_to_string(&path).expect("corpus/esctest-expected-pass.txt");
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// The gate. Both directions, the corpus law: a pinned pass that stops passing is a
/// regression, and an unpinned pass is a promotion the pin file must record.
#[test]
fn esctest_conformance_matches_the_pinned_expectations() {
    let results = run_esctest(GATE_INCLUDE, Duration::from_secs(600));
    let expected = expected_passes();

    let mut regressions = Vec::new();
    for name in &expected {
        match results.get(name) {
            Some(Outcome::Passed) => {}
            other => regressions.push(format!("{name}: {other:?}")),
        }
    }

    let mut promotions = Vec::new();
    for (name, outcome) in &results {
        if *outcome == Outcome::Passed && !expected.iter().any(|e| e == name) {
            promotions.push(name.clone());
        }
    }

    assert!(
        regressions.is_empty(),
        "pinned esctest passes no longer pass (REGRESSION):\n{}",
        regressions.join("\n")
    );
    assert!(
        promotions.is_empty(),
        "esctest tests now pass but are not pinned -- promote them into \
         corpus/esctest-expected-pass.txt:\n{}",
        promotions.join("\n")
    );
}

/// Authoring tool: the whole suite, results printed for triage. Run manually:
///
///     cargo test -p ruuah-vt-pty --test esctest -- --ignored print_esctest --nocapture
#[test]
#[ignore = "authoring tool; prints rather than asserts"]
fn print_esctest_results() {
    let results = run_esctest(".", Duration::from_secs(3600));
    let mut counts = BTreeMap::new();
    for (name, outcome) in &results {
        println!("{outcome:?}\t{name}");
        *counts.entry(format!("{outcome:?}")).or_insert(0usize) += 1;
    }
    println!("--- {counts:?} over {} tests", results.len());
}
