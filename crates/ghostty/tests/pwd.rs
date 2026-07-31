//! Purpose: pin what libghostty-vt does with OSC 7 and the pwd it stores, measured rather
//!   than assumed.
//! Public surface: none, this is a test.
//! Why this file: the pwd is an ABI observable (`GHOSTTY_TERMINAL_DATA_PWD`), so unlike
//!   OSC 52 or OSC 8 it has a real oracle and the corpus can pin it. Before ruuah-vt
//!   implements anything, the rules the corpus will encode are read off the library here --
//!   probe for WHAT, source for WHY, both discharged before a line of core is written.
//! NOT responsible for: ruuah-vt's own behaviour, or any comparison between the two.
//! Test strategy: write the sequences a shell actually emits and read the pwd back through
//!   the same ABI getter the differential harness uses.
//!
//! Source read alongside these probes, 2026-07-31:
//! `../ruuah/src/terminal/stream_terminal.zig` `reportPwd` -- the payload is stored RAW,
//! never parsed or validated, truncated at 4096 bytes; and `../ruuah/src/terminal/
//! Terminal.zig` `setPwd` (empty clears) / `fullReset` (RIS clears).

use ruuah_vt_ghostty::Terminal;

fn pwd_after(bytes: &[u8]) -> Vec<u8> {
    let mut terminal = Terminal::new(20, 4).expect("terminal creation");
    terminal.write(bytes);
    terminal.pwd().expect("pwd")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("probe payloads are ASCII")
}

#[test]
fn nothing_reported_means_an_empty_pwd() {
    assert_eq!(pwd_after(b"hello"), Vec::<u8>::new());
}

#[test]
fn osc7_stores_the_uri_verbatim() {
    let pwd = pwd_after(b"\x1b]7;file://host/Users/orel/src\x1b\\");
    assert_eq!(text(&pwd), "file://host/Users/orel/src");
}

/// BEL is the terminator shells actually emit (zsh writes `\a` from `precmd`), so a parser
/// that only accepts ST would look fine in every test here and fail on every real prompt.
#[test]
fn a_bel_terminator_works_too() {
    let pwd = pwd_after(b"\x1b]7;file:///tmp\x07");
    assert_eq!(text(&pwd), "file:///tmp");
}

/// The library does not parse the payload, so a value that is not a URI at all is still
/// stored. This is the rule that makes "validate it first" a wrong implementation.
#[test]
fn a_payload_that_is_not_a_uri_is_stored_anyway() {
    assert_eq!(text(&pwd_after(b"\x1b]7;/plain/path\x1b\\")), "/plain/path");
    assert_eq!(text(&pwd_after(b"\x1b]7;banana\x1b\\")), "banana");
}

/// Percent-encoding survives untouched -- decoding is the embedder's job. A core that
/// helpfully decoded here would diverge on every path containing a space.
#[test]
fn percent_encoding_is_not_decoded() {
    let pwd = pwd_after(b"\x1b]7;file:///Users/orel/My%20Code\x1b\\");
    assert_eq!(text(&pwd), "file:///Users/orel/My%20Code");
}

#[test]
fn an_empty_payload_clears_a_previously_reported_pwd() {
    let pwd = pwd_after(b"\x1b]7;file:///tmp\x1b\\\x1b]7;\x1b\\");
    assert_eq!(pwd, Vec::<u8>::new());
}

#[test]
fn a_second_report_replaces_the_first() {
    let pwd = pwd_after(b"\x1b]7;file:///one\x1b\\\x1b]7;file:///two\x1b\\");
    assert_eq!(text(&pwd), "file:///two");
}

/// RIS clears it (`fullReset`). The pwd is terminal state, not screen state, so this is
/// the only sequence that drops it without a new report.
#[test]
fn ris_clears_the_pwd() {
    let pwd = pwd_after(b"\x1b]7;file:///tmp\x1b\\\x1bc");
    assert_eq!(pwd, Vec::<u8>::new());
}

/// It lives on `Terminal`, not on a `Screen`, so switching buffers cannot disturb it --
/// and a report made ON the alternate screen is still there after coming back.
#[test]
fn the_pwd_is_terminal_global_across_the_alternate_screen() {
    let both_ways =
        pwd_after(b"\x1b]7;file:///primary\x1b\\\x1b[?1049h\x1b]7;file:///alt\x1b\\\x1b[?1049l");
    assert_eq!(text(&both_ways), "file:///alt");

    let survives = pwd_after(b"\x1b]7;file:///primary\x1b\\\x1b[?1049h");
    assert_eq!(text(&survives), "file:///primary");
}

/// DECSTR is a soft reset and does NOT touch the pwd -- worth pinning because esctest
/// sends one before every test, and because our own DECSTR is already a documented
/// divergence, so its blast radius has to be known rather than guessed.
#[test]
fn decstr_does_not_clear_the_pwd() {
    let pwd = pwd_after(b"\x1b]7;file:///tmp\x1b\\\x1b[!p");
    assert_eq!(text(&pwd), "file:///tmp");
}

/// THE LIMIT IS THE OSC PARSER'S, NOT `reportPwd`'S, AND IT DROPS RATHER THAN TRUNCATES.
///
/// Reading the source alone gets this wrong in both halves. `reportPwd` truncates at 4096
/// and looks authoritative -- but it is unreachable dead code for OSC 7, because
/// `osc.zig`'s parser captures into a fixed `[MAX_BUF]u8` with `MAX_BUF = 2048` and only
/// commands given an allocator (OSC 52) may exceed it. Binary-searched against the real
/// library on 2026-07-31: 2047 bytes of payload are stored whole and 2048 stores nothing.
/// 2047 rather than 2048 because the capture is NUL-terminated and the sentinel needs the
/// last byte.
#[test]
fn the_payload_limit_is_2047_bytes_and_the_cliff_is_a_drop() {
    let report = |n: usize| {
        let mut bytes = b"\x1b]7;".to_vec();
        bytes.extend(std::iter::repeat_n(b'a', n));
        bytes.extend_from_slice(b"\x1b\\");
        pwd_after(&bytes)
    };

    assert_eq!(report(2047).len(), 2047, "the largest payload stored whole");
    assert_eq!(report(2048), Vec::<u8>::new(), "one past it stores nothing");
    assert_eq!(report(5000), Vec::<u8>::new(), "and it never truncates to 4096");
}

/// An over-long report is a NO-OP, not a clear: it never reaches `setPwd`, so the previous
/// value survives, the parser recovers for the next command, and the dropped payload does
/// not spill into the grid as text. Three distinct wrong implementations die here --
/// clear-on-overflow, stay-broken-after-overflow, and print-the-leftovers.
#[test]
fn an_over_long_report_leaves_the_previous_pwd_alone() {
    let mut terminal = Terminal::new(20, 4).expect("terminal creation");
    terminal.write(b"\x1b]7;file:///kept\x1b\\");

    let mut bytes = b"\x1b]7;".to_vec();
    bytes.extend(std::iter::repeat_n(b'a', 3000));
    bytes.extend_from_slice(b"\x1b\\");
    terminal.write(&bytes);
    assert_eq!(text(&terminal.pwd().expect("pwd")), "file:///kept");

    terminal.write(b"\x1b]7;file:///after\x1b\\");
    assert_eq!(text(&terminal.pwd().expect("pwd")), "file:///after");

    let snapshot = terminal.snapshot().expect("snapshot");
    let row0: String = snapshot.grid[0].cells.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(row0.trim_end(), "", "the dropped payload must not print");
}

/// The header's callback doc names OSC 9 (ConEmu CurrentDir) and OSC 1337 as pwd sources
/// as well. Whether THIS build routes them there is a measurement, not a reading: the
/// answer decides whether ruuah-vt owes them an implementation or the corpus owes them a
/// named divergence. Recorded as measured on 2026-07-31 against `oracle.lock`'s build --
/// if either flips, the oracle moved and the corpus notes must be re-read.
#[test]
fn measured_what_the_other_documented_pwd_sources_do() {
    let osc9 = pwd_after(b"\x1b]9;9;/tmp/from-conemu\x1b\\");
    let osc1337 = pwd_after(b"\x1b]1337;CurrentDir=/tmp/from-iterm\x1b\\");

    assert_eq!(
        (text(&osc9), text(&osc1337)),
        ("/tmp/from-conemu".to_string(), "/tmp/from-iterm".to_string()),
    );
}
