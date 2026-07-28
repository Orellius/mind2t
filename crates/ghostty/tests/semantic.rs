//! Purpose: pin what libghostty-vt does with OSC 133, measured rather than assumed.
//! Public surface: none, this is a test.
//! Why this file: slice 5.6 needs an oracle for semantic prompt state, and an oracle is
//!   only usable once its rules are written down. Every assertion here was read off a
//!   probe against the real library on 2026-07-28 and then confirmed in
//!   `ruuah/src/terminal/Terminal.zig`, so the two halves of the project rule -- probe for
//!   WHAT, source for WHY -- are both discharged before the core implements anything.
//! NOT responsible for: ruuah-vt's own behaviour, or any comparison between the two.
//! Test strategy: drive the library with the sequences a shell actually emits, and assert
//!   on the snapshot's semantic layer.

use ruuah_vt_ghostty::Terminal;
use ruuah_vt_snapshot::{RowSemantic, Semantic, Snapshot};

fn snapshot_of(cols: u16, rows: u16, bytes: &[u8]) -> Snapshot {
    let mut terminal = Terminal::with_scrollback(cols, rows, 100).expect("terminal creation");
    terminal.write(bytes);
    terminal.snapshot().expect("snapshot")
}

/// The semantic content of row `y`, one entry per column, for compact assertions.
fn semantics(snap: &Snapshot, y: usize) -> Vec<Semantic> {
    snap.grid[y].cells.iter().map(|c| c.semantic).collect()
}

fn run(kind: Semantic, len: usize) -> Vec<Semantic> {
    vec![kind; len]
}

#[test]
fn a_terminal_that_never_saw_osc_133_reports_everything_as_output() {
    let snap = snapshot_of(8, 2, b"hello");

    assert_eq!(semantics(&snap, 0), run(Semantic::Output, 8));
    assert_eq!(snap.grid[0].semantic_prompt, RowSemantic::None);
}

#[test]
fn prompt_start_marks_the_cells_and_the_row() {
    let snap = snapshot_of(8, 2, b"\x1b]133;A\x07$ ");

    assert_eq!(snap.grid[0].cells[0].semantic, Semantic::Prompt);
    assert_eq!(snap.grid[0].cells[1].semantic, Semantic::Prompt);
    // Only cells actually written take the mark; the rest of the row is untouched.
    assert_eq!(snap.grid[0].cells[2].semantic, Semantic::Output);
    assert_eq!(snap.grid[0].semantic_prompt, RowSemantic::Prompt);
}

#[test]
fn b_switches_the_cells_to_input_without_changing_the_row() {
    let snap = snapshot_of(12, 2, b"\x1b]133;A\x07$ \x1b]133;B\x07ls");

    assert_eq!(
        semantics(&snap, 0)[..4],
        [
            Semantic::Prompt,
            Semantic::Prompt,
            Semantic::Input,
            Semantic::Input
        ]
    );
    assert_eq!(snap.grid[0].semantic_prompt, RowSemantic::Prompt);
}

#[test]
fn c_switches_back_to_output() {
    let snap = snapshot_of(12, 2, b"\x1b]133;A\x07$ \x1b]133;B\x07ls\x1b]133;C\x07ok");

    assert_eq!(
        semantics(&snap, 0)[..6],
        [
            Semantic::Prompt,
            Semantic::Prompt,
            Semantic::Input,
            Semantic::Input,
            Semantic::Output,
            Semantic::Output
        ]
    );
}

/// `A` is "fresh line, then new prompt": it moves to a new line first unless the cursor is
/// already at the left margin (`semanticPromptFreshLine` in Terminal.zig).
#[test]
fn a_does_a_fresh_line_only_when_the_cursor_has_moved() {
    let moved = snapshot_of(8, 3, b"xy\x1b]133;A\x07$");
    assert_eq!(moved.grid[0].semantic_prompt, RowSemantic::None);
    assert_eq!(moved.grid[1].semantic_prompt, RowSemantic::Prompt);

    let at_margin = snapshot_of(8, 3, b"xy\r\n\x1b]133;A\x07$");
    assert_eq!(at_margin.grid[1].semantic_prompt, RowSemantic::Prompt);
    assert_eq!(at_margin.cursor.y, 1);
}

/// A newline taken while the cursor is still in a non-output state marks the row it lands
/// on as a continuation. This is the workaround for shells that never send `k=s`, and it is
/// why a row can be a prompt continuation while holding input cells.
#[test]
fn a_newline_in_a_prompt_state_marks_the_next_row_as_a_continuation() {
    let snap = snapshot_of(12, 3, b"\x1b]133;A\x07$ \x1b]133;B\x07one\r\ntwo");

    assert_eq!(snap.grid[0].semantic_prompt, RowSemantic::Prompt);
    assert_eq!(
        snap.grid[1].semantic_prompt,
        RowSemantic::PromptContinuation
    );
    assert_eq!(semantics(&snap, 1)[..3], run(Semantic::Input, 3)[..]);
}

/// `I` starts input that ends at end-of-line, so `index()` resets the cursor to output
/// instead of marking a continuation. This is the one place `B` and `I` diverge.
#[test]
fn clear_at_eol_input_ends_at_the_newline_where_plain_input_does_not() {
    let explicit = snapshot_of(12, 3, b"\x1b]133;A\x07$ \x1b]133;B\x07one\r\ntwo");
    assert_eq!(
        explicit.grid[1].semantic_prompt,
        RowSemantic::PromptContinuation
    );
    assert_eq!(explicit.grid[1].cells[0].semantic, Semantic::Input);

    let at_eol = snapshot_of(12, 3, b"\x1b]133;A\x07$ \x1b]133;I\x07one\r\ntwo");
    assert_eq!(at_eol.grid[1].semantic_prompt, RowSemantic::None);
    assert_eq!(at_eol.grid[1].cells[0].semantic, Semantic::Output);
}

/// A soft wrap preserves the cursor's semantic state across the row boundary, because
/// `printWrap` saves it around the `index()` call and restores it afterwards.
#[test]
fn input_survives_a_soft_wrap() {
    let snap = snapshot_of(4, 3, b"\x1b]133;A\x07$ \x1b]133;B\x07abcdef");

    assert!(snap.grid[0].wrap);
    assert!(snap.grid[1].wrap_continuation);
    assert_eq!(snap.grid[1].cells[0].semantic, Semantic::Input);
    assert_eq!(
        snap.grid[1].semantic_prompt,
        RowSemantic::PromptContinuation
    );
}

/// The same wrap under `I`: the state is restored, so the cells stay input, but the row is
/// not marked a continuation because `index()` cleared that before `printWrap` restored.
#[test]
fn a_soft_wrap_under_clear_at_eol_keeps_the_cells_but_not_the_row_mark() {
    let snap = snapshot_of(4, 3, b"\x1b]133;A\x07$ \x1b]133;I\x07abcdef");

    assert_eq!(snap.grid[1].cells[0].semantic, Semantic::Input);
    assert_eq!(snap.grid[1].semantic_prompt, RowSemantic::None);
}

/// The fish workaround: `C` arriving at column 0 on a row that carries a prompt mark
/// un-marks it, on the assumption that the shell is overwriting the prompt with output.
#[test]
fn end_of_input_at_column_zero_clears_the_row_mark() {
    let at_zero = snapshot_of(12, 3, b"\x1b]133;A\x07$ p\r\n\x1b]133;C\x07out");
    assert_eq!(at_zero.grid[1].semantic_prompt, RowSemantic::None);

    let mid_row = snapshot_of(12, 3, b"\x1b]133;A\x07$ \x1b]133;B\x07ls\x1b]133;C\x07out");
    assert_eq!(mid_row.grid[0].semantic_prompt, RowSemantic::Prompt);
}

/// `P;k=c` marks a continuation row directly, which is what a shell with a PS2 emits.
#[test]
fn an_explicit_continuation_prompt_marks_the_row_as_a_continuation() {
    let snap = snapshot_of(
        12,
        3,
        b"\x1b]133;A\x07$ \x1b]133;B\x07x\r\n\x1b]133;P;k=c\x07> ",
    );

    assert_eq!(snap.grid[0].semantic_prompt, RowSemantic::Prompt);
    assert_eq!(
        snap.grid[1].semantic_prompt,
        RowSemantic::PromptContinuation
    );
    assert_eq!(snap.grid[1].cells[0].semantic, Semantic::Prompt);
}

/// Semantics are written per cell at print time, so overwriting a cell replaces its mark.
#[test]
fn overwriting_a_prompt_cell_replaces_its_mark() {
    let snap = snapshot_of(12, 2, b"\x1b]133;A\x07$ abc\x1b]133;C\x07\rXY");

    assert_eq!(
        semantics(&snap, 0)[..5],
        [
            Semantic::Output,
            Semantic::Output,
            Semantic::Prompt,
            Semantic::Prompt,
            Semantic::Prompt
        ]
    );
    // The row mark is not recomputed from the cells, so it survives the overwrite.
    assert_eq!(snap.grid[0].semantic_prompt, RowSemantic::Prompt);
}

/// Erasing a cell clears its semantic content along with its text, but leaves the row mark.
#[test]
fn erasing_a_line_clears_cell_semantics_and_keeps_the_row_mark() {
    let snap = snapshot_of(12, 2, b"\x1b]133;A\x07$ abc\x1b[2K");

    assert_eq!(semantics(&snap, 0), run(Semantic::Output, 12));
    assert_eq!(snap.grid[0].semantic_prompt, RowSemantic::Prompt);
}

/// Both layers survive the crossing into scrollback. Measured after a reporting bug made it
/// look otherwise: `Snapshot`'s `Display` printed no per-cell layers for history rows, so
/// the dump showed a bare row while `diff` would have seen the real values.
#[test]
fn both_layers_survive_the_crossing_into_history() {
    let snap = snapshot_of(4, 2, b"\x1b]133;A\x07$ abcdef\r\nz\r\ny\r\nx");

    assert_eq!(snap.history.len(), 3);
    assert_eq!(snap.history[0].semantic_prompt, RowSemantic::Prompt);
    assert!(
        snap.history[0]
            .cells
            .iter()
            .all(|c| c.semantic == Semantic::Prompt)
    );
    assert_eq!(
        snap.history[1].semantic_prompt,
        RowSemantic::PromptContinuation
    );
}

#[test]
fn the_alternate_screen_starts_from_a_clean_slate() {
    let snap = snapshot_of(12, 2, b"\x1b]133;A\x07$ p\x1b[?1049h");

    assert_eq!(snap.grid[0].semantic_prompt, RowSemantic::None);
    assert_eq!(semantics(&snap, 0), run(Semantic::Output, 12));
}
