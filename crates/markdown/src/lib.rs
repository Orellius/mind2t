//! Purpose: lay out a GitHub-flavoured-markdown pipe table as fixed-width box-drawing text.
//! Public surface: `detect_table`, `Table`, `Align`, `layout`.
//! Why this crate exists: an agent CLI (Claude Code, etc.) writes literal `| a | b |` rows
//!   wider than the pane and the terminal wraps them into pipe soup. This crate is the
//!   PRESENTATION half of the fix - rows of text in, a laid-out table out - with no
//!   knowledge of a grid, a pty or a screen. Detection scope (which rows are eligible: a
//!   completed OSC 133 output region on the primary screen, off by default) and the actual
//!   draw pass belong to the host and are NOT built here; see docs/BACKLOG-2026.md P1.
//! NOT responsible for: deciding which terminal rows to feed in, drawing pixels, or
//!   preserving the original bytes for copy - the caller keeps the grid unchanged and draws
//!   this output as an overlay, exactly as the backlog's sketch specifies.
//! Test strategy: pure unit tests, no pty, no fixture files - every case is a literal string.

/// Column alignment, from a separator cell's colon placement (`:--`, `--:`, `:--:`, `--`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Center,
}

/// A table extracted from plain text lines: header row, alignments, body rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub header: Vec<String>,
    pub aligns: Vec<Align>,
    pub rows: Vec<Vec<String>>,
    /// How many input lines this table consumed, starting at the header. The caller uses
    /// this to know which lines to replace and which to leave alone.
    pub line_count: usize,
}

/// Splits a single GFM pipe-table row into cells. Leading/trailing pipes are optional and
/// stripped; a `\|` inside a cell escapes the pipe rather than splitting on it, because a
/// naive `split('|')` would break the first table cell containing a literal pipe.
fn split_row(line: &str) -> Vec<String> {
    let line = line.trim();
    let inner = line
        .strip_prefix('|')
        .unwrap_or(line)
        .strip_suffix('|')
        .unwrap_or(line.strip_prefix('|').unwrap_or(line));
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'|') {
            cur.push('|');
            chars.next();
        } else if c == '|' {
            cells.push(cur.trim().to_string());
            cur = String::new();
        } else {
            cur.push(c);
        }
    }
    cells.push(cur.trim().to_string());
    cells
}

/// A line is a candidate table row if it contains an unescaped, non-quoted pipe. This is
/// deliberately loose - the SEPARATOR row is what actually confirms a table, matching how
/// GFM itself decides (CommonMark table extension: a table exists only if row 2 is a valid
/// delimiter row for row 1's column count).
fn looks_like_row(line: &str) -> bool {
    let t = line.trim();
    t.contains('|') && !t.is_empty()
}

/// Parses a separator row (`| --- | :--: | ---: |`) into per-column alignment, or `None` if
/// the line is not a valid separator - each cell must be only `-`, `:` and at least one `-`.
fn parse_separator(line: &str) -> Option<Vec<Align>> {
    let cells = split_row(line);
    if cells.is_empty() {
        return None;
    }
    cells
        .iter()
        .map(|c| {
            let c = c.trim();
            if c.is_empty() || !c.chars().all(|ch| ch == '-' || ch == ':') {
                return None;
            }
            if !c.contains('-') {
                return None;
            }
            let left = c.starts_with(':');
            let right = c.ends_with(':');
            Some(match (left, right) {
                (true, true) => Align::Center,
                (false, true) => Align::Right,
                _ => Align::Left,
            })
        })
        .collect()
}

/// Finds a table starting at `lines[0]`, or `None` if `lines` does not open with one.
/// The caller scans forward one line at a time and calls this at each candidate start -
/// this function never looks backward and never skips lines on its own.
pub fn detect_table(lines: &[&str]) -> Option<Table> {
    if lines.len() < 2 || !looks_like_row(lines[0]) {
        return None;
    }
    let aligns = parse_separator(lines[1])?;
    let header = split_row(lines[0]);
    if header.len() != aligns.len() {
        return None;
    }

    let mut rows = Vec::new();
    let mut i = 2;
    while i < lines.len() && looks_like_row(lines[i]) {
        let mut cells = split_row(lines[i]);
        cells.resize(header.len(), String::new());
        cells.truncate(header.len());
        rows.push(cells);
        i += 1;
    }

    Some(Table {
        header,
        aligns,
        rows,
        line_count: i,
    })
}

fn pad(cell: &str, width: usize, align: Align) -> String {
    let len = cell.chars().count();
    let gap = width.saturating_sub(len);
    match align {
        Align::Left => format!(" {cell}{} ", " ".repeat(gap)),
        Align::Right => format!(" {}{cell} ", " ".repeat(gap)),
        Align::Center => {
            let left = gap / 2;
            let right = gap - left;
            format!(" {}{cell}{} ", " ".repeat(left), " ".repeat(right))
        }
    }
}

/// Truncates a cell to `width` characters, marking loss with a trailing `…` - never a
/// silent drop, because a table this crate cannot fit honestly should say so.
fn fit(cell: &str, width: usize) -> String {
    if cell.chars().count() <= width {
        return cell.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut s: String = cell.chars().take(width.saturating_sub(1)).collect();
    s.push('…');
    s
}

/// Lays out `table` as box-drawing text no wider than `max_width` columns. Column widths
/// are distributed evenly across `max_width` first, THEN content is measured against that
/// budget and truncated - the alternative (measure content first, shrink to fit) would let
/// one wide row dictate every column's width and starve the others, so the terminal pane's
/// width is always the constraint that wins.
pub fn layout(table: &Table, max_width: usize) -> Vec<String> {
    let n = table.header.len().max(1);
    // Every column gets `| ` + content + ` ` before the next separator, so n+1 separators
    // and 2*n padding spaces are fixed overhead; the remainder is split across columns.
    let overhead = n + 1 + 2 * n;
    let budget = max_width.saturating_sub(overhead);
    let base = budget / n;
    let extra = budget % n;
    let natural: Vec<usize> = (0..n)
        .map(|i| {
            let header_len = table.header.get(i).map(|s| s.chars().count()).unwrap_or(0);
            let body_max = table
                .rows
                .iter()
                .filter_map(|r| r.get(i))
                .map(|s| s.chars().count())
                .max()
                .unwrap_or(0);
            header_len.max(body_max)
        })
        .collect();
    // Columns that fit within their even share keep their natural width; the space that
    // frees up is handed to whichever column still needs it, capped by the total budget -
    // a table of short cells should not be padded out to the full pane width.
    let mut widths = vec![0usize; n];
    let mut spare = 0usize;
    for i in 0..n {
        let share = base + usize::from(i < extra);
        if natural[i] <= share {
            widths[i] = natural[i].max(1);
            spare += share - widths[i];
        } else {
            widths[i] = share;
        }
    }
    for i in 0..n {
        if natural[i] > widths[i] && spare > 0 {
            let want = (natural[i] - widths[i]).min(spare);
            widths[i] += want;
            spare -= want;
        }
    }

    let sep = |left: &str, mid: &str, right: &str, fill: char| {
        let mut s = left.to_string();
        for (i, w) in widths.iter().enumerate() {
            s.push_str(&fill.to_string().repeat(w + 2));
            s.push_str(if i + 1 == widths.len() { right } else { mid });
        }
        s
    };
    let data_row = |cells: &[String]| {
        let mut s = "│".to_string();
        for (i, w) in widths.iter().enumerate() {
            let text = fit(cells.get(i).map(|s| s.as_str()).unwrap_or(""), *w);
            s.push_str(&pad(&text, *w, table.aligns.get(i).copied().unwrap_or(Align::Left)));
            s.push('│');
        }
        s
    };

    let mut out = Vec::with_capacity(table.rows.len() + 4);
    out.push(sep("┌", "┬", "┐", '─'));
    out.push(data_row(&table.header));
    out.push(sep("├", "┼", "┤", '─'));
    for row in &table.rows {
        out.push(data_row(row));
    }
    out.push(sep("└", "┴", "┘", '─'));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_not_a_table() {
        let lines = ["just some output", "no pipes here"];
        assert!(detect_table(&lines).is_none());
    }

    #[test]
    fn a_pipe_with_no_valid_separator_is_not_a_table() {
        // second line has no dashes at all - looks like a row, is not a delimiter row.
        let lines = ["| a | b |", "| x | y |"];
        assert!(detect_table(&lines).is_none());
    }

    #[test]
    fn basic_table_is_detected_with_alignment() {
        let lines = [
            "| Name | Score |",
            "| :--- | ----: |",
            "| alice | 10 |",
            "| bob | 200 |",
            "not a row",
        ];
        let t = detect_table(&lines).expect("should detect");
        assert_eq!(t.header, vec!["Name", "Score"]);
        assert_eq!(t.aligns, vec![Align::Left, Align::Right]);
        assert_eq!(t.rows, vec![vec!["alice", "10"], vec!["bob", "200"]]);
        assert_eq!(t.line_count, 4);
    }

    #[test]
    fn a_literal_pipe_in_a_cell_does_not_split_it() {
        let lines = [r"| cmd | example |", "| --- | --- |", r"| ls | \| grep foo |"];
        let t = detect_table(&lines).expect("should detect");
        assert_eq!(t.rows[0][1], "| grep foo");
    }

    #[test]
    fn a_short_row_is_padded_with_empty_cells_not_dropped() {
        let lines = ["| a | b | c |", "| - | - | - |", "| 1 | 2 |"];
        let t = detect_table(&lines).expect("should detect");
        assert_eq!(t.rows[0], vec!["1", "2", ""]);
    }

    #[test]
    fn layout_produces_aligned_rectangular_output() {
        let lines = ["| Name | Score |", "| :--- | ----: |", "| alice | 10 |"];
        let t = detect_table(&lines).unwrap();
        let out = layout(&t, 40);
        // every row is the same width, which is what makes it a rectangle on screen.
        let w = out[0].chars().count();
        assert!(out.iter().all(|l| l.chars().count() == w));
        assert!(out[0].starts_with('┌') && out[0].ends_with('┐'));
        assert!(out.last().unwrap().starts_with('└'));
        assert!(out[1].contains("Name"));
        // right alignment: "10" should sit against the right edge of its column, not the left.
        assert!(out[3].contains(" 10│") || out[3].contains(" 10 │"));
    }

    #[test]
    fn a_cell_too_wide_for_the_budget_is_truncated_with_an_ellipsis_not_silently_dropped() {
        let lines = [
            "| Name | Description |",
            "| --- | --- |",
            "| x | this is a very long description that will not fit |",
        ];
        let t = detect_table(&lines).unwrap();
        let out = layout(&t, 20);
        assert!(out[3].contains('…'), "expected truncation marker in: {}", out[3]);
        let w = out[0].chars().count();
        assert!(w <= 20, "layout exceeded max_width: {w} > 20 ({})", out[0]);
    }

    #[test]
    fn narrow_columns_do_not_get_padded_to_fill_a_wide_pane() {
        // "ok"/"no" are short; a naive even-split would pad them out to ~35 chars each.
        let lines = ["| a | b |", "| - | - |", "| ok | no |"];
        let t = detect_table(&lines).unwrap();
        let out = layout(&t, 80);
        // the actual content row should not have absurd internal padding.
        assert!(out[1].len() < 20, "header row grew unreasonably wide: {:?}", out[1]);
    }

    #[test]
    fn zero_width_separator_lines_are_rejected() {
        assert!(parse_separator("|  |  |").is_none());
        assert!(parse_separator("| :: |").is_none());
    }

    #[test]
    fn detect_table_requires_the_header_and_separator_column_counts_to_match() {
        let lines = ["| a | b | c |", "| - | - |"];
        assert!(detect_table(&lines).is_none());
    }
}
