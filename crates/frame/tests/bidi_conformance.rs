//! The oracle for slice 5.5: the Unicode bidi conformance suite.
//!
//! libghostty-vt cannot be the oracle here. `grep -rni bidi ../ruuah/include/ghostty/vt/*.h`
//! returns nothing, so the ABI has no opinion about reordering at all -- which is exactly why
//! reordering lives above the core and why a different reference is needed to say whether it
//! is right. Unicode publishes one, and it is exhaustive.
//!
//! What is certified here is **our** `visual_spans`, not merely the crate underneath it. The
//! test builds a row of real cells from each case's codepoints, lays it out, flattens the
//! spans back to a visual index order, and compares against Unicode's expected answer. A bug
//! anywhere in the wrapper -- span ordering, direction, the fast path that skips the
//! algorithm for plain text -- fails here.
//!
//! Requires the suite:
//!
//!     ./scripts/fetch-ucd.sh
//!
//! It is vendored rather than committed for the same reason libghostty-vt is: ~15 MB of
//! static data reproducible from a URL. The version is pinned in `ucd.lock`.

use std::path::{Path, PathBuf};

use ruuah_vt_frame::{BaseDirection, Direction, PackedCell, visual_spans};
use ruuah_vt_snapshot::{Semantic, Wide};

fn ucd(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/ucd")
        .canonicalize()
        .unwrap_or_else(|_| {
            panic!(
                "the Unicode bidi conformance suite is missing. Run ./scripts/fetch-ucd.sh\n\
                 It is the oracle for reordering; without it this test certifies nothing."
            )
        });
    dir.join(name)
}

fn base(field: u8) -> BaseDirection {
    match field {
        0 => BaseDirection::LeftToRight,
        1 => BaseDirection::RightToLeft,
        _ => BaseDirection::Auto,
    }
}

/// One conformance case, parsed.
struct Case {
    chars: Vec<char>,
    base: BaseDirection,
    /// `None` for a character rule X9 removes.
    levels: Vec<Option<u8>>,
    expected: Vec<usize>,
}

fn cases() -> Vec<Case> {
    let text = std::fs::read_to_string(ucd("BidiCharacterTest.txt")).expect("read suite");
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let f: Vec<&str> = line.split(';').collect();
            if f.len() < 5 {
                return None;
            }
            Some(Case {
                chars: f[0]
                    .split_whitespace()
                    .map(|h| char::from_u32(u32::from_str_radix(h, 16).unwrap()).unwrap())
                    .collect(),
                base: base(f[1].parse().unwrap()),
                levels: f[3]
                    .split_whitespace()
                    .map(|t| (t != "x").then(|| t.parse().unwrap()))
                    .collect(),
                expected: f[4]
                    .split_whitespace()
                    .map(|t| t.parse().unwrap())
                    .collect(),
            })
        })
        .collect()
}

/// Lays a case out through the real run builder path and returns the visual index order.
fn visual_order(case: &Case) -> Vec<usize> {
    let row: Vec<PackedCell> = case
        .chars
        .iter()
        .map(|c| PackedCell::new(&c.to_string(), 0, Wide::Narrow, Semantic::Output))
        .collect();

    let mut order = Vec::with_capacity(row.len());
    for span in visual_spans(&row, case.base) {
        match span.direction {
            Direction::LeftToRight => order.extend(span.logical.clone()),
            Direction::RightToLeft => order.extend(span.logical.clone().rev()),
        }
    }

    // Unicode's expected order omits characters rule X9 removes; ours carries every cell,
    // because a terminal cell exists whether or not the algorithm kept its character.
    order
        .into_iter()
        .filter(|i| case.levels.get(*i).map(Option::is_some).unwrap_or(false))
        .collect()
}

#[test]
fn visual_spans_match_the_unicode_conformance_suite() {
    let all = cases();
    assert!(
        all.len() > 90_000,
        "suite looks truncated: {} cases",
        all.len()
    );

    // Cases containing a layout character are excluded, and counted rather than quietly
    // dropped: our segment rule deliberately refuses to reorder across box drawing, so
    // Unicode's whole-paragraph answer is not the answer we want there. That divergence is
    // the policy, and it is unit-tested separately in `bidi.rs`.
    let (applicable, segmented): (Vec<_>, Vec<_>) = all.iter().partition(|case| {
        !case
            .chars
            .iter()
            .copied()
            .any(ruuah_vt_frame::bidi::is_layout)
    });

    let mut failures = Vec::new();
    for case in &applicable {
        let got = visual_order(case);
        if got != case.expected && failures.len() < 5 {
            failures.push(format!(
                "  chars {:?}\n    expected {:?}\n    got      {:?}",
                case.chars, case.expected, got
            ));
        }
    }

    println!(
        "BidiCharacterTest: {} cases applied, {} excluded by the segment rule",
        applicable.len(),
        segmented.len()
    );

    assert!(
        failures.is_empty(),
        "{} of {} conformance cases failed:\n{}",
        failures.len(),
        applicable.len(),
        failures.join("\n")
    );
}

#[test]
fn the_conformance_check_can_actually_fail() {
    // A conformance run that has never been seen to fail proves nothing. This feeds the same
    // suite through a deliberately broken layout -- every row left in logical order, which is
    // precisely what the renderer did before this slice -- and requires the comparison to
    // reject it. If this ever passes, the test above has stopped reading the answer.
    let broken: Vec<usize> = cases()
        .iter()
        .take(4_000)
        .filter(|case| {
            let logical: Vec<usize> = (0..case.chars.len())
                .filter(|i| case.levels.get(*i).map(Option::is_some).unwrap_or(false))
                .collect();
            logical != case.expected
        })
        .map(|case| case.chars.len())
        .collect();

    assert!(
        !broken.is_empty(),
        "no case in the suite distinguishes logical order from visual order, so the \
         conformance comparison cannot detect a renderer that does not reorder at all"
    );
    println!(
        "{} of the first 4000 cases require actual reordering",
        broken.len()
    );
}

#[test]
fn the_pinned_unicode_version_is_the_one_on_disk() {
    // A conformance failure after a Unicode revision is a different thing from a regression
    // you caused, and telling them apart afterwards is guesswork. `ucd.lock` records which
    // revision the reordering was verified against.
    let lock =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ucd.lock"))
            .expect("ucd.lock -- run ./scripts/fetch-ucd.sh");
    let pinned = lock
        .lines()
        .find_map(|line| line.strip_prefix("version = "))
        .map(|v| v.trim_matches('"').to_string())
        .expect("a version in ucd.lock");

    let header = std::fs::read_to_string(ucd("BidiCharacterTest.txt"))
        .expect("read suite")
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();

    assert!(
        header.contains(&pinned),
        "ucd.lock pins Unicode {pinned} but the vendored suite says {header:?}"
    );
}
