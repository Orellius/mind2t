//! Purpose: decide whether two snapshots agree, and name precisely where they do not.
//! Public surface: `Difference` and `diff`.
//! Why this file: a differential harness is only as useful as the specificity of its
//!   report. "the grids differ" is worthless; `cell[3,12].text` is a lead.
//! NOT responsible for: deciding which side is right. The oracle is right by definition
//!   here, but this function is symmetric and does not encode that.
//! Test strategy: unit tests below, covering agreement, each field class, and the
//!   short-circuit when dimensions disagree.

use crate::grid::{Cell, Row, Snapshot, Style, describe_bytes, describe_rgb, describe_style};

/// One named disagreement between two snapshots.
///
/// `path` is a stable, greppable locator such as `cursor.x`, `row[3].wrap`, or
/// `cell[3,12].style.bold`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    pub path: String,
    pub oracle: String,
    pub candidate: String,
}

impl Difference {
    fn new(path: impl Into<String>, oracle: impl ToString, candidate: impl ToString) -> Difference {
        Difference {
            path: path.into(),
            oracle: oracle.to_string(),
            candidate: candidate.to_string(),
        }
    }
}

impl std::fmt::Display for Difference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: oracle={} candidate={}", self.path, self.oracle, self.candidate)
    }
}

/// Every way `candidate` disagrees with `oracle`, in reading order.
///
/// Mismatched dimensions short-circuit: comparing cells across different geometries
/// produces thousands of derived differences that all restate the one real cause.
pub fn diff(oracle: &Snapshot, candidate: &Snapshot) -> Vec<Difference> {
    let mut out = Vec::new();

    if oracle.cols != candidate.cols {
        out.push(Difference::new("dims.cols", oracle.cols, candidate.cols));
    }
    if oracle.rows != candidate.rows {
        out.push(Difference::new("dims.rows", oracle.rows, candidate.rows));
    }
    if !out.is_empty() {
        return out;
    }

    if oracle.screen != candidate.screen {
        out.push(Difference::new(
            "screen",
            format!("{:?}", oracle.screen),
            format!("{:?}", candidate.screen),
        ));
    }

    if oracle.pwd != candidate.pwd {
        out.push(Difference::new(
            "pwd",
            describe_bytes(&oracle.pwd),
            describe_bytes(&candidate.pwd),
        ));
    }

    // The OSC colour state. Dynamic colours first, then the palette entry by entry --
    // one difference per index, so a case that overrides index 1 and a wholesale
    // default-table drift read differently in the report.
    let dynamic: [(&str, Option<crate::grid::Rgb>, Option<crate::grid::Rgb>); 3] = [
        ("colors.foreground", oracle.colors.foreground, candidate.colors.foreground),
        ("colors.background", oracle.colors.background, candidate.colors.background),
        ("colors.cursor", oracle.colors.cursor, candidate.colors.cursor),
    ];
    for (path, o, c) in dynamic {
        if o != c {
            out.push(Difference::new(path, describe_rgb(o), describe_rgb(c)));
        }
    }
    if oracle.colors.palette.len() != candidate.colors.palette.len() {
        out.push(Difference::new(
            "colors.palette.len",
            oracle.colors.palette.len(),
            candidate.colors.palette.len(),
        ));
    } else {
        for (i, (o, c)) in
            oracle.colors.palette.iter().zip(candidate.colors.palette.iter()).enumerate()
        {
            if o != c {
                out.push(Difference::new(
                    format!("colors.palette[{i}]"),
                    describe_rgb(Some(*o)),
                    describe_rgb(Some(*c)),
                ));
            }
        }
    }

    if oracle.modes.bracketed_paste != candidate.modes.bracketed_paste {
        out.push(Difference::new(
            "modes.bracketed_paste",
            oracle.modes.bracketed_paste,
            candidate.modes.bracketed_paste,
        ));
    }

    if oracle.modes.synchronized_output != candidate.modes.synchronized_output {
        out.push(Difference::new(
            "modes.synchronized_output",
            oracle.modes.synchronized_output,
            candidate.modes.synchronized_output,
        ));
    }

    // The nine mouse-mode raw bits, uniformly: one entry per bit so a mutation test can
    // kill each comparison individually (`a_mode_only_one_side_set_is_reported`).
    let mouse_bits: [(&str, bool, bool); 13] = [
        ("modes.cursor_keys", oracle.modes.cursor_keys, candidate.modes.cursor_keys),
        ("modes.keypad_keys", oracle.modes.keypad_keys, candidate.modes.keypad_keys),
        (
            "modes.ignore_keypad_with_numlock",
            oracle.modes.ignore_keypad_with_numlock,
            candidate.modes.ignore_keypad_with_numlock,
        ),
        ("modes.alt_esc_prefix", oracle.modes.alt_esc_prefix, candidate.modes.alt_esc_prefix),
        ("modes.mouse_event_x10", oracle.modes.mouse_event_x10, candidate.modes.mouse_event_x10),
        (
            "modes.mouse_event_normal",
            oracle.modes.mouse_event_normal,
            candidate.modes.mouse_event_normal,
        ),
        (
            "modes.mouse_event_button",
            oracle.modes.mouse_event_button,
            candidate.modes.mouse_event_button,
        ),
        ("modes.mouse_event_any", oracle.modes.mouse_event_any, candidate.modes.mouse_event_any),
        (
            "modes.mouse_format_utf8",
            oracle.modes.mouse_format_utf8,
            candidate.modes.mouse_format_utf8,
        ),
        ("modes.mouse_format_sgr", oracle.modes.mouse_format_sgr, candidate.modes.mouse_format_sgr),
        (
            "modes.mouse_format_urxvt",
            oracle.modes.mouse_format_urxvt,
            candidate.modes.mouse_format_urxvt,
        ),
        (
            "modes.mouse_format_sgr_pixels",
            oracle.modes.mouse_format_sgr_pixels,
            candidate.modes.mouse_format_sgr_pixels,
        ),
        (
            "modes.mouse_alternate_scroll",
            oracle.modes.mouse_alternate_scroll,
            candidate.modes.mouse_alternate_scroll,
        ),
    ];
    for (path, oracle_bit, candidate_bit) in mouse_bits {
        if oracle_bit != candidate_bit {
            out.push(Difference::new(path, oracle_bit, candidate_bit));
        }
    }

    diff_cursor(oracle, candidate, &mut out);

    // History length first: a shorter or longer history shifts every row, so comparing rows
    // pairwise after a length mismatch reports the shift N times instead of once.
    if oracle.history.len() != candidate.history.len() {
        out.push(Difference::new(
            "history.len",
            oracle.history.len(),
            candidate.history.len(),
        ));
    } else {
        for (y, (o_row, c_row)) in oracle
            .history
            .iter()
            .zip(candidate.history.iter())
            .enumerate()
        {
            diff_row_at(
                &format!("history[{y}]"),
                &format!("history_cell[{y},"),
                o_row,
                c_row,
                &mut out,
            );
        }
    }

    for (y, (o_row, c_row)) in oracle.grid.iter().zip(candidate.grid.iter()).enumerate() {
        diff_row(y, o_row, c_row, &mut out);
    }
    if oracle.grid.len() != candidate.grid.len() {
        out.push(Difference::new(
            "grid.len",
            oracle.grid.len(),
            candidate.grid.len(),
        ));
    }

    diff_damage(oracle, candidate, &mut out);

    out
}

/// Compares what each side says a renderer would have to repaint.
///
/// Only when both sides observed it. A case that does not ask for damage leaves `None` on
/// both, and one that does gets it from both, so a mismatch here is a real disagreement
/// rather than a case half-configured.
fn diff_damage(oracle: &Snapshot, candidate: &Snapshot, out: &mut Vec<Difference>) {
    let (Some(o), Some(c)) = (&oracle.damage, &candidate.damage) else {
        if oracle.damage.is_some() != candidate.damage.is_some() {
            out.push(Difference::new(
                "damage.observed",
                oracle.damage.is_some(),
                candidate.damage.is_some(),
            ));
        }
        return;
    };

    if o.global != c.global {
        out.push(Difference::new(
            "damage.global",
            format!("{:?}", o.global),
            format!("{:?}", c.global),
        ));
    }
    if o.rows.len() != c.rows.len() {
        out.push(Difference::new("damage.rows.len", o.rows.len(), c.rows.len()));
    }
    for (y, (o_row, c_row)) in o.rows.iter().zip(c.rows.iter()).enumerate() {
        if o_row != c_row {
            out.push(Difference::new(&format!("damage.row[{y}]"), *o_row, *c_row));
        }
    }
}

fn diff_cursor(oracle: &Snapshot, candidate: &Snapshot, out: &mut Vec<Difference>) {
    let (o, c) = (&oracle.cursor, &candidate.cursor);
    if o.x != c.x {
        out.push(Difference::new("cursor.x", o.x, c.x));
    }
    if o.y != c.y {
        out.push(Difference::new("cursor.y", o.y, c.y));
    }
    if o.pending_wrap != c.pending_wrap {
        out.push(Difference::new(
            "cursor.pending_wrap",
            o.pending_wrap,
            c.pending_wrap,
        ));
    }
    if o.visible != c.visible {
        out.push(Difference::new("cursor.visible", o.visible, c.visible));
    }
    diff_style("cursor.style", &o.style, &c.style, out);
}

fn diff_row(y: usize, oracle: &Row, candidate: &Row, out: &mut Vec<Difference>) {
    diff_row_at(&format!("row[{y}]"), &format!("cell[{y},"), oracle, candidate, out)
}

/// Compares one row under a caller-supplied path prefix, so active-area rows and history
/// rows produce differently-located differences from the same code.
fn diff_row_at(
    row_path: &str,
    cell_prefix: &str,
    oracle: &Row,
    candidate: &Row,
    out: &mut Vec<Difference>,
) {
    if oracle.wrap != candidate.wrap {
        out.push(Difference::new(
            format!("{row_path}.wrap"),
            oracle.wrap,
            candidate.wrap,
        ));
    }
    if oracle.wrap_continuation != candidate.wrap_continuation {
        out.push(Difference::new(
            format!("{row_path}.wrap_continuation"),
            oracle.wrap_continuation,
            candidate.wrap_continuation,
        ));
    }
    if oracle.semantic_prompt != candidate.semantic_prompt {
        out.push(Difference::new(
            format!("{row_path}.semantic_prompt"),
            format!("{:?}", oracle.semantic_prompt),
            format!("{:?}", candidate.semantic_prompt),
        ));
    }
    for (x, (o_cell, c_cell)) in oracle.cells.iter().zip(candidate.cells.iter()).enumerate() {
        diff_cell(&format!("{cell_prefix}{x}]"), o_cell, c_cell, out);
    }
    if oracle.cells.len() != candidate.cells.len() {
        out.push(Difference::new(
            format!("{row_path}.cells.len"),
            oracle.cells.len(),
            candidate.cells.len(),
        ));
    }
}

fn diff_cell(path: &str, oracle: &Cell, candidate: &Cell, out: &mut Vec<Difference>) {
    if oracle.text != candidate.text {
        out.push(Difference::new(
            format!("{path}.text"),
            quote(&oracle.text),
            quote(&candidate.text),
        ));
    }
    if oracle.wide != candidate.wide {
        out.push(Difference::new(
            format!("{path}.wide"),
            format!("{:?}", oracle.wide),
            format!("{:?}", candidate.wide),
        ));
    }
    if oracle.semantic != candidate.semantic {
        out.push(Difference::new(
            format!("{path}.semantic"),
            format!("{:?}", oracle.semantic),
            format!("{:?}", candidate.semantic),
        ));
    }
    diff_style(&format!("{path}.style"), &oracle.style, &candidate.style, out);
}

/// Reports the whole style as one difference rather than one per attribute: a single
/// SGR mistake flips several flags at once, and eight lines saying the same thing
/// buries the next real finding.
fn diff_style(path: &str, oracle: &Style, candidate: &Style, out: &mut Vec<Difference>) {
    if oracle == candidate {
        return;
    }
    out.push(Difference::new(
        path,
        describe_style(oracle),
        describe_style(candidate),
    ));
}

/// Renders cell text so an empty cell and a cell holding a space are distinguishable,
/// and so a codepoint that does not render legibly still appears in the report.
fn quote(text: &str) -> String {
    if text.is_empty() {
        return "<empty>".to_string();
    }
    let escaped: String = text
        .chars()
        .map(|c| {
            if c.is_control() || !c.is_ascii() {
                format!("\\u{{{:04x}}}", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect();
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{
        Color, Cursor, Damage, Dirty, RowSemantic, Screen, Semantic, Underline, Wide,
        describe_color,
    };

    fn snapshot(cols: u16, rows: u16) -> Snapshot {
        Snapshot {
            cols,
            rows,
            screen: Screen::Primary,
            cursor: Cursor {
                x: 0,
                y: 0,
                pending_wrap: false,
                visible: true,
                style: Style::DEFAULT,
            },
            grid: (0..rows)
                .map(|_| Row {
                    wrap: false,
                    wrap_continuation: false,
                    semantic_prompt: RowSemantic::None,
                    cells: (0..cols).map(|_| Cell::blank()).collect(),
                })
                .collect(),
            modes: crate::grid::Modes::default(),
            history: Vec::new(),
            damage: None,
            pwd: Vec::new(),
            colors: crate::grid::Colors::default(),
        }
    }

    fn row_of(cols: u16, text: &str) -> Row {
        Row {
            wrap: false,
            wrap_continuation: false,
            semantic_prompt: RowSemantic::None,
            cells: (0..cols)
                .map(|x| Cell {
                    text: text.chars().nth(usize::from(x)).map(String::from).unwrap_or_default(),
                    ..Cell::blank()
                })
                .collect(),
        }
    }

    // The three controls below exist because the audit's mutations M2, M4 and M5 proved
    // their comparisons could be deleted with every gate still green -- the S1 lesson
    // applied to the fields it missed (finding 25). Each pair differs in exactly one field,
    // so the assertion is on the full difference list, not a needle in it.

    #[test]
    fn a_screen_disagreement_is_reported() {
        let a = snapshot(4, 2);
        let mut b = a.clone();
        b.screen = Screen::Alternate;

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "screen");
    }

    /// The mode has no grid effect at all, so this control is the ONLY thing that dies
    /// if the comparison is deleted -- there is no corpus case that could catch it by
    /// accident the way a content comparison might be caught.
    #[test]
    fn a_bracketed_paste_disagreement_is_reported() {
        let a = snapshot(4, 2);
        let mut b = a.clone();
        b.modes.bracketed_paste = true;

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "modes.bracketed_paste");
    }

    /// Same law again, and the reason this file changed at all for OSC 7: the pwd leaves
    /// no mark on the grid, so before this comparison existed a core that ignored OSC 7
    /// entirely scored a perfect MATCH on every case that reports one.
    #[test]
    fn a_pwd_disagreement_is_reported() {
        let a = snapshot(4, 2);
        let mut b = a.clone();
        b.pwd = b"file:///tmp".to_vec();

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "pwd");
        assert_eq!(found[0].oracle, "<empty>");
        assert_eq!(found[0].candidate, "\"file:///tmp\"");
    }

    /// The payload is compared as BYTES. Two values that differ only outside UTF-8 -- or
    /// only in percent-encoding, which a helpful implementation might "normalise" away --
    /// must still be reported, so the comparison cannot be satisfied by decoding both
    /// sides into the same string.
    #[test]
    fn a_pwd_that_differs_only_in_non_utf8_bytes_is_reported() {
        let mut a = snapshot(4, 2);
        let mut b = a.clone();
        a.pwd = b"file:///tmp/\xff".to_vec();
        b.pwd = b"file:///tmp/\xfe".to_vec();

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "pwd");
        assert_ne!(found[0].oracle, found[0].candidate);
    }

    /// The colour comparisons' controls, one per path. Same blind-spot law as pwd:
    /// cells carry palette INDICES, so a core that ignores OSC 4/10/11/12 entirely
    /// leaves every cell comparison green -- these four are the only guards.
    #[test]
    fn a_dynamic_colour_disagreement_is_reported_per_path() {
        use crate::grid::Rgb;
        for (path, set) in [
            ("colors.foreground", 0usize),
            ("colors.background", 1),
            ("colors.cursor", 2),
        ] {
            let a = snapshot(4, 2);
            let mut b = a.clone();
            let value = Some(Rgb { r: 0xF0, g: 0x00, b: 0x0D });
            match set {
                0 => b.colors.foreground = value,
                1 => b.colors.background = value,
                _ => b.colors.cursor = value,
            }
            let found = diff(&a, &b);
            assert_eq!(found.len(), 1, "{path}: {found:?}");
            assert_eq!(found[0].path, path);
            assert_eq!(found[0].oracle, "<unset>");
            assert_eq!(found[0].candidate, "#f0000d");
        }
    }

    #[test]
    fn a_palette_entry_disagreement_names_its_index() {
        use crate::grid::Rgb;
        let a = snapshot(4, 2);
        let mut b = a.clone();
        b.colors.palette[1] = Rgb { r: 0xFF, g: 0x00, b: 0x00 };
        b.colors.palette[231] = Rgb { r: 0x00, g: 0x00, b: 0x01 };

        let found = diff(&a, &b);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].path, "colors.palette[1]");
        assert_eq!(found[0].oracle, "#cc6666", "index 1 default is the measured Tomorrow red");
        assert_eq!(found[0].candidate, "#ff0000");
        assert_eq!(found[1].path, "colors.palette[231]");
    }

    /// Same law as bracketed paste: no grid effect, so only this control guards the
    /// comparison itself.
    #[test]
    fn a_synchronized_output_disagreement_is_reported() {
        let a = snapshot(4, 2);
        let mut b = a.clone();
        b.modes.synchronized_output = true;

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "modes.synchronized_output");
    }

    /// One flip per mouse raw bit, each expected to surface as exactly its own path:
    /// deleting any single entry from the comparison table kills exactly one of these
    /// assertions, which is what makes the table's nine entries individually real.
    #[test]
    fn each_mouse_mode_bit_disagreement_is_reported_under_its_own_path() {
        let flips: [(&str, fn(&mut crate::grid::Modes)); 13] = [
            ("modes.cursor_keys", |m| m.cursor_keys = true),
            ("modes.keypad_keys", |m| m.keypad_keys = true),
            // 1035 and 1036 default ON; their disagreement direction is a clear.
            ("modes.ignore_keypad_with_numlock", |m| m.ignore_keypad_with_numlock = false),
            ("modes.alt_esc_prefix", |m| m.alt_esc_prefix = false),
            ("modes.mouse_event_x10", |m| m.mouse_event_x10 = true),
            ("modes.mouse_event_normal", |m| m.mouse_event_normal = true),
            ("modes.mouse_event_button", |m| m.mouse_event_button = true),
            ("modes.mouse_event_any", |m| m.mouse_event_any = true),
            ("modes.mouse_format_utf8", |m| m.mouse_format_utf8 = true),
            ("modes.mouse_format_sgr", |m| m.mouse_format_sgr = true),
            ("modes.mouse_format_urxvt", |m| m.mouse_format_urxvt = true),
            ("modes.mouse_format_sgr_pixels", |m| m.mouse_format_sgr_pixels = true),
            // 1007 defaults ON, so its disagreement direction is a clear.
            ("modes.mouse_alternate_scroll", |m| m.mouse_alternate_scroll = false),
        ];
        for (path, flip) in flips {
            let a = snapshot(4, 2);
            let mut b = a.clone();
            flip(&mut b.modes);

            let found = diff(&a, &b);
            assert_eq!(found.len(), 1, "{path}: {found:?}");
            assert_eq!(found[0].path, path);
        }
    }

    #[test]
    fn a_cursor_visibility_disagreement_is_reported() {
        let a = snapshot(4, 2);
        let mut b = a.clone();
        b.cursor.visible = false;

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "cursor.visible");
    }

    #[test]
    fn a_cursor_style_disagreement_is_reported() {
        let a = snapshot(4, 2);
        let mut b = a.clone();
        b.cursor.style.bold = true;

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "cursor.style");
    }

    #[test]
    fn history_length_short_circuits_instead_of_shifting_every_row() {
        // A history that is one row longer shifts every row against its partner. Reporting
        // that as N cell differences buries the one real cause.
        let mut a = snapshot(4, 2);
        let mut b = a.clone();
        a.history = vec![row_of(4, "old"), row_of(4, "new")];
        b.history = vec![row_of(4, "new")];

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "history.len");
    }

    #[test]
    fn a_history_cell_is_located_in_history_not_the_active_area() {
        let mut a = snapshot(4, 2);
        let mut b = a.clone();
        a.history = vec![row_of(4, "abc")];
        b.history = vec![row_of(4, "abX")];

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "history_cell[0,2].text");
        assert_eq!(found[0].oracle, "'c'");
    }

    #[test]
    fn identical_history_is_not_reported() {
        let mut a = snapshot(4, 2);
        a.history = vec![row_of(4, "same")];
        let b = a.clone();
        assert_eq!(diff(&a, &b), vec![]);
    }

    /// The control for the semantic layer: snapshots identical but for OSC 133 content must
    /// still disagree, or slice 5.6's sixteen corpus cases would pass against a core that
    /// had quietly stopped tracking semantics.
    #[test]
    fn a_cell_differing_only_in_semantic_content_is_reported() {
        let a = snapshot(4, 2);
        let mut b = a.clone();
        b.grid[0].cells[2].semantic = Semantic::Input;

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "cell[0,2].semantic");
        assert_eq!(found[0].oracle, "Output");
        assert_eq!(found[0].candidate, "Input");
    }

    #[test]
    fn a_row_differing_only_in_its_prompt_mark_is_reported() {
        let a = snapshot(4, 2);
        let mut b = a.clone();
        b.grid[1].semantic_prompt = RowSemantic::PromptContinuation;

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "row[1].semantic_prompt");
        assert_eq!(found[0].candidate, "PromptContinuation");
    }

    /// The row mark is not a summary of the cells: deriving one from the other hides this.
    #[test]
    fn the_row_mark_and_the_cell_marks_are_compared_independently() {
        let mut a = snapshot(4, 2);
        a.grid[0].semantic_prompt = RowSemantic::Prompt;
        let mut b = a.clone();
        b.grid[0].semantic_prompt = RowSemantic::None;
        b.grid[0].cells[0].semantic = Semantic::Prompt;

        let found = diff(&a, &b);
        assert_eq!(found.len(), 2, "{found:?}");
    }

    #[test]
    fn identical_snapshots_agree() {
        let a = snapshot(10, 3);
        let b = a.clone();
        assert_eq!(diff(&a, &b), vec![]);
    }

    #[test]
    fn a_changed_cell_is_named_by_coordinate() {
        let a = snapshot(10, 3);
        let mut b = a.clone();
        b.grid[1].cells[4].text = "x".to_string();

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "cell[1,4].text");
        assert_eq!(found[0].oracle, "<empty>");
        assert_eq!(found[0].candidate, "'x'");
    }

    #[test]
    fn an_empty_cell_is_distinguishable_from_a_space() {
        let a = snapshot(4, 1);
        let mut b = a.clone();
        b.grid[0].cells[0].text = " ".to_string();

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].oracle, "<empty>");
        assert_eq!(found[0].candidate, "' '");
    }

    #[test]
    fn cursor_fields_are_reported_separately() {
        let a = snapshot(10, 3);
        let mut b = a.clone();
        b.cursor.x = 5;
        b.cursor.pending_wrap = true;

        let paths: Vec<String> = diff(&a, &b).into_iter().map(|d| d.path).collect();
        assert_eq!(paths, ["cursor.x", "cursor.pending_wrap"]);
    }

    /// The four controls below exist because mutation proved the ones above did not cover
    /// these fields: `diff_damage` could be replaced with a no-op, and the `screen`,
    /// `cursor.visible` and `cursor.style` comparisons deleted, with every gate still green.
    /// The corpus structurally cannot catch that -- 90 of its cases expect MATCH, so removing
    /// a comparison only makes more snapshots agree.
    #[test]
    fn a_hidden_cursor_is_reported() {
        let a = snapshot(4, 2);
        let mut b = a.clone();
        b.cursor.visible = false;

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "cursor.visible");
        assert_eq!(found[0].oracle, "true");
        assert_eq!(found[0].candidate, "false");
    }

    /// The pen the cursor will print with is state a grid comparison cannot otherwise see:
    /// an SGR that never gets written to a cell leaves no trace anywhere else.
    #[test]
    fn the_cursor_pen_is_compared() {
        let a = snapshot(4, 2);
        let mut b = a.clone();
        b.cursor.style.bold = true;
        b.cursor.style.fg = Color::Palette(4);

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "cursor.style");
        assert_eq!(found[0].oracle, "default");
        assert_eq!(found[0].candidate, "fg=palette(4) bold");
    }

    /// Two grids can be cell-for-cell identical and still be different screens. Without this,
    /// an alt-screen bug that leaves the right content behind is invisible.
    #[test]
    fn the_alternate_screen_is_not_mistaken_for_the_primary() {
        let a = snapshot(4, 2);
        let mut b = a.clone();
        b.screen = Screen::Alternate;

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "screen");
        assert_eq!(found[0].oracle, "Primary");
        assert_eq!(found[0].candidate, "Alternate");
    }

    #[test]
    fn every_damage_layer_is_compared() {
        // Each of the three is a separate failure a renderer would show: a wrong global state
        // skips or forces a whole frame, a wrong row count misaligns every flag after it, and
        // a wrong row flag leaves one stale line on screen.
        let mut a = snapshot(4, 2);
        a.damage = Some(Damage {
            global: Dirty::Partial,
            rows: vec![true, false],
        });

        let mut global = a.clone();
        global.damage = Some(Damage {
            global: Dirty::Full,
            rows: vec![true, false],
        });
        let found = diff(&a, &global);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "damage.global");
        assert_eq!(found[0].candidate, "Full");

        let mut row = a.clone();
        row.damage = Some(Damage {
            global: Dirty::Partial,
            rows: vec![true, true],
        });
        let found = diff(&a, &row);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "damage.row[1]");

        let mut count = a.clone();
        count.damage = Some(Damage {
            global: Dirty::Partial,
            rows: vec![true],
        });
        let found = diff(&a, &count);
        assert_eq!(found[0].path, "damage.rows.len");
    }

    /// One side observing damage and the other not is a harness fault, not a terminal
    /// disagreement, and it must be reported rather than silently skipped.
    #[test]
    fn damage_observed_on_only_one_side_is_reported() {
        let a = snapshot(4, 2);
        let mut b = a.clone();
        b.damage = Some(Damage {
            global: Dirty::None,
            rows: vec![false, false],
        });

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "damage.observed");
        assert_eq!(found[0].oracle, "false");
        assert_eq!(found[0].candidate, "true");
    }

    #[test]
    fn identical_damage_is_not_reported() {
        let mut a = snapshot(4, 2);
        a.damage = Some(Damage {
            global: Dirty::Partial,
            rows: vec![false, true],
        });
        let b = a.clone();
        assert_eq!(diff(&a, &b), vec![]);
    }

    #[test]
    fn a_style_mismatch_is_one_difference_not_one_per_attribute() {
        let a = snapshot(4, 1);
        let mut b = a.clone();
        b.grid[0].cells[0].style.bold = true;
        b.grid[0].cells[0].style.italic = true;
        b.grid[0].cells[0].style.fg = Color::Palette(3);

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "cell[0,0].style");
        assert_eq!(found[0].oracle, "default");
        assert_eq!(found[0].candidate, "fg=palette(3) bold italic");
    }

    #[test]
    fn mismatched_dimensions_short_circuit() {
        let a = snapshot(10, 3);
        let b = snapshot(20, 3);

        let found = diff(&a, &b);
        assert_eq!(found.len(), 1, "must not cascade into per-cell noise");
        assert_eq!(found[0].path, "dims.cols");
    }

    #[test]
    fn row_wrap_flags_are_compared() {
        let a = snapshot(4, 2);
        let mut b = a.clone();
        b.grid[0].wrap = true;
        b.grid[1].wrap_continuation = true;

        let paths: Vec<String> = diff(&a, &b).into_iter().map(|d| d.path).collect();
        assert_eq!(paths, ["row[0].wrap", "row[1].wrap_continuation"]);
    }

    #[test]
    fn wide_and_underline_survive_the_round_trip() {
        let a = snapshot(4, 1);
        let mut b = a.clone();
        b.grid[0].cells[0].wide = Wide::SpacerTail;
        b.grid[0].cells[1].style.underline = Underline::Curly;

        let found = diff(&a, &b);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].path, "cell[0,0].wide");
        assert_eq!(found[0].candidate, "SpacerTail");
        assert_eq!(found[1].candidate, "underline=Curly");
    }

    #[test]
    fn non_ascii_cell_text_is_escaped_legibly() {
        let a = snapshot(4, 1);
        let mut b = a.clone();
        b.grid[0].cells[0].text = "\u{5d0}".to_string();

        let found = diff(&a, &b);
        assert_eq!(found[0].candidate, "'\\u{05d0}'");
    }

    #[test]
    fn describe_color_covers_every_variant() {
        assert_eq!(describe_color(Color::Default), "default");
        assert_eq!(describe_color(Color::Palette(9)), "palette(9)");
        assert_eq!(
            describe_color(Color::Rgb { r: 1, g: 2, b: 255 }),
            "#0102ff"
        );
    }
}
