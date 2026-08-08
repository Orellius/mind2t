//! Purpose: derive a selection range from a point, and format it as clipboard text.
//! Public surface: `select`, `format`, `Rules`; reached through `Terminal::select`.
//! Why this file: selection is the one observable in this core that is a QUERY rather than
//!   state. No byte stream produces it, so it hangs off the grid rather than off the parser,
//!   and keeping it out of `screen.rs` keeps the mutation path free of read-only logic.
//! NOT responsible for: tracking a live selection across writes, mouse gestures, or drawing
//!   a highlight. A selection here is a value derived on demand and immediately stale.
//! Test strategy: the differential corpus (`select-*` cases). Every rule below was read out
//!   of the oracle's own `Screen.zig` and then confirmed by measurement, never inferred from
//!   a header - the two disagree, and the header is the one that is wrong.

use mind2t_vt_snapshot::{Point, Row, Selection, Wide};

/// The codepoints that bound a word, and the ones trimmed off a line.
///
/// Ported verbatim from the oracle's `selection_codepoints.zig`, because a selection that
/// splits words differently from the terminal it is a drop-in for is a selection people
/// notice within a minute of using it.
///
/// Read the set carefully before "fixing" it: `.`, `/`, `-` and `_` are NOT boundaries, so a
/// path, a filename and a flag select whole. That is the behaviour, not an oversight - and
/// it is the single most useful thing double-click does in a terminal.
pub struct Rules;

impl Rules {
    /// ` \t'"│`|:;,()[]{}<>$` plus NUL.
    pub const WORD_BOUNDARIES: &'static [char] = &[
        '\0', ' ', '\t', '\'', '"', '\u{2502}', '`', '|', ':', ';', ',', '(', ')', '[', ']', '{',
        '}', '<', '>', '$',
    ];

    /// Trimmed from both ends of a line selection.
    pub const LINE_WHITESPACE: &'static [char] = &['\0', ' ', '\t'];

    fn is_word_boundary(c: char) -> bool {
        Rules::WORD_BOUNDARIES.contains(&c)
    }

    fn is_line_whitespace(c: char) -> bool {
        Rules::LINE_WHITESPACE.contains(&c)
    }
}

/// A cell has text when something was written into it.
///
/// The spacer tail of a wide cell is NOT text: it is the second half of its neighbour, and
/// the oracle answers "no selectable word" for a point on one. Measured - a probe at x=1 of
/// `你好` returns GHOSTTY_NO_VALUE.
///
/// The `SpacerTail` clause is REDUNDANT against this core's own grids and is kept anyway.
/// Measured: `Grid::cell_text` returns an empty string for a tail (its codepoint is 0), so
/// the emptiness test alone already answers correctly, and a mutant deleting the clause
/// survived the whole corpus. It stays because `has_text` takes any `Row`, including ones a
/// consumer builds itself, and because the two facts - "no codepoint" and "not a character" -
/// are independent reasons that happen to coincide here. `a_spacer_tail_is_never_selectable`
/// is the control that makes the clause do work.
fn has_text(row: &Row, x: u16) -> bool {
    match row.cells.get(usize::from(x)) {
        None => false,
        Some(cell) => cell.wide != Wide::SpacerTail && !cell.text.is_empty(),
    }
}

/// The first codepoint of a cell, which is what every boundary rule tests. A cluster's
/// combining marks never change its class.
fn lead_char(row: &Row, x: u16) -> Option<char> {
    row.cells.get(usize::from(x)).and_then(|cell| cell.text.chars().next())
}

/// One step right through the buffer, crossing into the next row at the end of this one.
///
/// Row-crossing is unconditional here and the wrap flag is tested by the CALLER, because the
/// two scans use it differently: going forward, the last cell of an unwrapped row is INCLUDED
/// and ends the scan; going backward, arriving at the last cell of an unwrapped row ends the
/// scan without it.
///
/// The forward asymmetry is load-bearing and a corpus case pins it
/// (`select-word-filling-a-whole-row`). The BACKWARD one is not, and saying so is the point:
/// a mutant that swapped the order of the two backward guards survived the entire corpus,
/// because both of them simply break, so which fires first cannot change the answer. The
/// order is kept only to read the same way as the oracle. An earlier version of this comment
/// claimed the backward asymmetry mattered; the mutant is what proved it wrong.
fn step_right(rows: &[Row], cols: u16, at: Point) -> Option<Point> {
    if at.x + 1 < cols {
        Some(Point { x: at.x + 1, y: at.y })
    } else if usize::from(at.y) + 1 < rows.len() {
        Some(Point { x: 0, y: at.y + 1 })
    } else {
        None
    }
}

fn step_left(rows: &[Row], cols: u16, at: Point) -> Option<Point> {
    let _ = rows;
    if at.x > 0 {
        Some(Point { x: at.x - 1, y: at.y })
    } else if at.y > 0 {
        Some(Point { x: cols - 1, y: at.y - 1 })
    } else {
        None
    }
}

/// The word under `at`, or `None` when there is nothing selectable there.
///
/// A word is a run of cells that are all boundary codepoints or all not - which is NOT the
/// same as "whitespace or not", despite what the oracle's own doc comment says. Its code
/// tests set membership, and `.` is not in the set. The comment is wrong; the code is the
/// contract.
pub fn select_word(rows: &[Row], cols: u16, at: Point) -> Option<Selection> {
    let row = rows.get(usize::from(at.y))?;
    if !has_text(row, at.x) {
        return None;
    }
    let expect_boundary = lead_char(row, at.x).map(Rules::is_word_boundary).unwrap_or(false);

    let mut end = at;
    let mut cursor = at;
    while let Some(next) = step_right(rows, cols, cursor) {
        let row = &rows[usize::from(next.y)];

        if !has_text(row, next.x) {
            break;
        }
        let this_boundary = lead_char(row, next.x).map(Rules::is_word_boundary).unwrap_or(false);
        if this_boundary != expect_boundary {
            break;
        }
        // The last cell of a row that does not soft-wrap ends the word AND belongs to it.
        if next.x == cols - 1 && !row.wrap {
            end = next;
            break;
        }
        end = next;
        cursor = next;
    }

    let mut start = at;
    let mut cursor = at;
    while let Some(prev) = step_left(rows, cols, cursor) {
        let row = &rows[usize::from(prev.y)];

        // Checked BEFORE the text test, unlike the forward scan: landing on the last cell of
        // an unwrapped row means we just crossed a hard line break upward.
        if prev.x == cols - 1 && !row.wrap {
            break;
        }
        if !has_text(row, prev.x) {
            break;
        }
        let this_boundary = lead_char(row, prev.x).map(Rules::is_word_boundary).unwrap_or(false);
        if this_boundary != expect_boundary {
            break;
        }
        start = prev;
        cursor = prev;
    }

    Some(Selection { start, end, rectangle: false })
}

/// The logical line under `at`, trimmed of leading and trailing whitespace.
///
/// "Logical" means soft wraps are followed in both directions and hard breaks are not, so a
/// command that wrapped over three rows selects as one line - which is the whole reason
/// triple-click is useful. Returns `None` when the line holds nothing but whitespace.
pub fn select_line(rows: &[Row], cols: u16, at: Point) -> Option<Selection> {
    if at.y as usize >= rows.len() {
        return None;
    }

    // Walk up while the row above soft-wraps into this one.
    let mut top = at.y;
    while top > 0 && rows[usize::from(top) - 1].wrap {
        top -= 1;
    }
    // Walk down while this row soft-wraps into the next.
    let mut bottom = at.y;
    while usize::from(bottom) + 1 < rows.len() && rows[usize::from(bottom)].wrap {
        bottom += 1;
    }

    let first = Point { x: 0, y: top };
    let last = Point { x: cols - 1, y: bottom };

    // Trim inward from both ends. A cell with no text is skipped rather than treated as a
    // boundary: an unwritten cell in the middle of a line is a hole, not an end.
    let mut start = first;
    loop {
        let row = &rows[usize::from(start.y)];
        let blank = !has_text(row, start.x)
            || lead_char(row, start.x).map(Rules::is_line_whitespace).unwrap_or(true);
        if !blank {
            break;
        }
        match step_right(rows, cols, start) {
            Some(next) if next.y <= bottom => start = next,
            _ => return None,
        }
    }

    let mut end = last;
    loop {
        let row = &rows[usize::from(end.y)];
        let blank = !has_text(row, end.x)
            || lead_char(row, end.x).map(Rules::is_line_whitespace).unwrap_or(true);
        if !blank {
            break;
        }
        match step_left(rows, cols, end) {
            Some(prev) if prev.y >= top => end = prev,
            _ => return None,
        }
    }

    Some(Selection { start, end, rectangle: false })
}

/// Every written cell in the buffer, scrollback included, trimmed of whitespace at both ends.
pub fn select_all(rows: &[Row], cols: u16) -> Option<Selection> {
    let mut start = None;
    'outer: for (y, row) in rows.iter().enumerate() {
        for x in 0..cols {
            if has_text(row, x)
                && !lead_char(row, x).map(Rules::is_line_whitespace).unwrap_or(true)
            {
                start = Some(Point { x, y: y as u16 });
                break 'outer;
            }
        }
    }
    let start = start?;

    let mut end = start;
    for (y, row) in rows.iter().enumerate() {
        for x in 0..cols {
            if has_text(row, x)
                && !lead_char(row, x).map(Rules::is_line_whitespace).unwrap_or(true)
            {
                end = Point { x, y: y as u16 };
            }
        }
    }

    Some(Selection { start, end, rectangle: false })
}

/// The clipboard text for a selection: plain, soft wraps unwrapped, trailing blanks trimmed.
///
/// The three options are not independent taste. `selection.h` names this exact combination as
/// the one matching Ghostty's own `Screen.selectionString()`, so it is what the operator gets
/// from cmd+C in the terminal Mind2t is replacing.
pub fn format(rows: &[Row], cols: u16, selection: &Selection) -> String {
    let (start, end) = ordered(selection);
    let mut out = String::new();

    for y in start.y..=end.y.min(rows.len().saturating_sub(1) as u16) {
        let row = &rows[usize::from(y)];
        let from = if y == start.y { start.x } else { 0 };
        let to = if y == end.y { end.x } else { cols - 1 };

        let mut line = String::new();
        for x in from..=to {
            match row.cells.get(usize::from(x)) {
                // A spacer tail contributes nothing: its glyph was already emitted by the
                // wide cell to its left, and adding a space here puts a hole in every CJK
                // string that survives the round trip looking almost right.
                Some(cell) if cell.wide == Wide::SpacerTail => {}
                // An unwritten cell is a space, and then gets trimmed if it is trailing.
                Some(cell) if cell.text.is_empty() => line.push(' '),
                Some(cell) => line.push_str(&cell.text),
                None => {}
            }
        }
        out.push_str(line.trim_end_matches(Rules::LINE_WHITESPACE));

        // A row that soft-wraps into the next is the SAME line: unwrapping is what makes a
        // copied command paste back as one command instead of as three broken fragments.
        if y < end.y && !row.wrap {
            out.push('\n');
        }
    }

    out
}

/// The endpoints in reading order. A gesture may produce them either way round, and the
/// oracle keeps whichever order it was given, so ordering is the reader's job.
fn ordered(selection: &Selection) -> (Point, Point) {
    let (a, b) = (selection.start, selection.end);
    if (b.y, b.x) < (a.y, a.x) { (b, a) } else { (a, b) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mind2t_vt_snapshot::{Cell, RowSemantic};

    fn row(text: &str, cols: u16, wrap: bool) -> Row {
        let mut cells: Vec<Cell> = text
            .chars()
            .map(|c| Cell { text: c.to_string(), ..Cell::blank() })
            .collect();
        cells.resize(usize::from(cols), Cell::blank());
        Row { wrap, wrap_continuation: false, semantic_prompt: RowSemantic::None, cells }
    }

    /// The rule the oracle's own doc comment gets wrong. Its code tests set membership, and
    /// `.` is not in the set, so a path selects whole.
    #[test]
    fn a_dot_does_not_bound_a_word() {
        let rows = [row("foo.bar baz", 20, false)];
        let found = select_word(&rows, 20, Point { x: 1, y: 0 }).unwrap();
        assert_eq!(found.start, Point { x: 0, y: 0 });
        assert_eq!(found.end, Point { x: 6, y: 0 });
    }

    /// The control for the `SpacerTail` clause in `has_text`, which the corpus cannot kill:
    /// this core writes a tail with no codepoint, so emptiness alone already answers there.
    /// A row built by hand can carry both, and then only the clause is left to say no.
    #[test]
    fn a_spacer_tail_is_never_selectable_even_carrying_text() {
        let mut rows = [row("ab", 4, false)];
        rows[0].cells[1].wide = Wide::SpacerTail;
        rows[0].cells[0].wide = Wide::Wide;

        assert!(select_word(&rows, 4, Point { x: 1, y: 0 }).is_none());
        assert!(select_word(&rows, 4, Point { x: 0, y: 0 }).is_some(), "its head still selects");
    }

    /// The other half: a tail must not contribute a character to the clipboard either, or
    /// every wide glyph comes back doubled.
    #[test]
    fn a_spacer_tail_contributes_nothing_to_the_text() {
        let mut rows = [row("ab", 4, false)];
        rows[0].cells[1].wide = Wide::SpacerTail;
        let whole =
            Selection { start: Point { x: 0, y: 0 }, end: Point { x: 3, y: 0 }, rectangle: false };
        assert_eq!(format(&rows, 4, &whole), "a");
    }

    #[test]
    fn a_colon_does_bound_a_word() {
        let rows = [row("host:port", 20, false)];
        let found = select_word(&rows, 20, Point { x: 1, y: 0 }).unwrap();
        assert_eq!(found.end, Point { x: 3, y: 0 });
    }

    /// Whitespace is a run of its own, not a boundary that snaps to a neighbour.
    #[test]
    fn a_run_of_spaces_selects_as_itself() {
        let rows = [row("ab   cd", 20, false)];
        let found = select_word(&rows, 20, Point { x: 3, y: 0 }).unwrap();
        assert_eq!(found.start, Point { x: 2, y: 0 });
        assert_eq!(found.end, Point { x: 4, y: 0 });
    }

    #[test]
    fn a_word_crosses_a_soft_wrap_and_stops_at_a_hard_one() {
        let soft = [row("abcde", 5, true), row("fghij", 5, false)];
        let found = select_word(&soft, 5, Point { x: 1, y: 0 }).unwrap();
        assert_eq!(found.end, Point { x: 4, y: 1 }, "a soft wrap is the same word");

        let hard = [row("abcde", 5, false), row("fghij", 5, false)];
        let found = select_word(&hard, 5, Point { x: 1, y: 0 }).unwrap();
        assert_eq!(found.end, Point { x: 4, y: 0 }, "a hard break ends it, inclusive");
        let found = select_word(&hard, 5, Point { x: 1, y: 1 }).unwrap();
        assert_eq!(found.start, Point { x: 0, y: 1 }, "and does not reach back upward");
    }

    #[test]
    fn text_unwraps_across_a_soft_wrap_and_breaks_across_a_hard_one() {
        let rows = [row("abcde", 5, true), row("fg", 5, false)];
        let all = Selection {
            start: Point { x: 0, y: 0 },
            end: Point { x: 1, y: 1 },
            rectangle: false,
        };
        assert_eq!(format(&rows, 5, &all), "abcdefg");

        let rows = [row("abc", 5, false), row("fg", 5, false)];
        assert_eq!(format(&rows, 5, &all), "abc\nfg");
    }

    #[test]
    fn formatting_trims_trailing_blanks_per_row() {
        let rows = [row("abc", 10, false)];
        let whole = Selection {
            start: Point { x: 0, y: 0 },
            end: Point { x: 9, y: 0 },
            rectangle: false,
        };
        assert_eq!(format(&rows, 10, &whole), "abc");
    }

    #[test]
    fn a_reversed_selection_formats_the_same_as_a_forward_one() {
        let rows = [row("abcde", 5, false)];
        let forward = Selection {
            start: Point { x: 0, y: 0 },
            end: Point { x: 4, y: 0 },
            rectangle: false,
        };
        let reversed = Selection {
            start: Point { x: 4, y: 0 },
            end: Point { x: 0, y: 0 },
            rectangle: false,
        };
        assert_eq!(format(&rows, 5, &forward), format(&rows, 5, &reversed));
    }
}
