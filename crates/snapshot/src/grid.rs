//! Purpose: the terminal state a differential test is allowed to observe.
//! Public surface: `Snapshot`, `Row`, `Cell`, `Cursor`, `Style`, `Color`, `Wide`,
//!   `Underline`, `Screen`, and a `Display` impl that renders a snapshot for human eyes.
//! Why this file: both implementations must agree on what "the grid" *is* before they
//!   can be compared, so the shape lives in one place that neither of them owns.
//! NOT responsible for: parsing, mutation, comparison (see `difference.rs`), or storage
//!   layout of a real terminal. A `Cell` here is deliberately convenient, not compact.
//! Test strategy: exercised through `difference.rs` tests and the corpus integration run.

use std::fmt;

/// Which screen buffer the snapshot was taken from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Primary,
    Alternate,
}

/// A cell's contribution to the horizontal advance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wide {
    /// Ordinary width-1 cell.
    Narrow,
    /// First half of a width-2 cell.
    Wide,
    /// Second half of a width-2 cell. Carries no text and is not rendered.
    SpacerTail,
    /// Filler at the end of a soft-wrapped row that could not fit a wide cell.
    SpacerHead,
}

/// Underline decoration, matching the SGR 4:n sub-parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Underline {
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// A colour slot in a style: unset, a palette index, or a direct RGB value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    Default,
    Palette(u8),
    Rgb { r: u8, g: u8, b: u8 },
}

/// Visual attributes applied to a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub underline_color: Color,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: Underline,
}

/// One grid cell.
///
/// `text` holds the whole grapheme cluster, not a single codepoint: a cell is not a
/// codepoint, and encoding that assumption here would make the harness unable to see
/// the class of bug it exists to catch. An empty string is a cell with no text, which
/// is distinct from a cell holding U+0020.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    pub wide: Wide,
    pub style: Style,
}

/// One grid row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// This row soft-wraps into the next one.
    pub wrap: bool,
    /// This row is the continuation of a soft-wrap from the previous one.
    pub wrap_continuation: bool,
    pub cells: Vec<Cell>,
}

/// Cursor position and the style newly printed cells will take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub x: u16,
    pub y: u16,
    /// The next printable character wraps before it is written (the DEC phantom state).
    pub pending_wrap: bool,
    pub visible: bool,
    pub style: Style,
}

/// The full observable state of a terminal at one instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub cols: u16,
    pub rows: u16,
    pub screen: Screen,
    pub cursor: Cursor,
    /// Active-area rows, top to bottom. Length is `rows`; each row holds `cols` cells.
    pub grid: Vec<Row>,
    /// Scrollback rows above the active area, oldest first.
    ///
    /// Separate from `grid` rather than prepended to it so the active area keeps stable
    /// coordinates: `cell[0,0]` means the same thing whether or not history exists, which
    /// keeps every pre-scrollback corpus case and its recorded diffs valid.
    pub history: Vec<Row>,
}

impl Style {
    pub const DEFAULT: Style = Style {
        fg: Color::Default,
        bg: Color::Default,
        underline_color: Color::Default,
        bold: false,
        italic: false,
        faint: false,
        blink: false,
        inverse: false,
        invisible: false,
        strikethrough: false,
        overline: false,
        underline: Underline::None,
    };

    pub fn is_default(&self) -> bool {
        *self == Style::DEFAULT
    }
}

impl Default for Style {
    fn default() -> Self {
        Style::DEFAULT
    }
}

impl Cell {
    pub fn blank() -> Cell {
        Cell {
            text: String::new(),
            wide: Wide::Narrow,
            style: Style::DEFAULT,
        }
    }
}

impl Snapshot {
    /// The row's text with trailing blanks removed, for legible reports.
    pub fn row_text(&self, y: usize) -> String {
        self.text_of(self.grid.get(y))
    }

    /// The same, for a scrollback row.
    pub fn history_text(&self, y: usize) -> String {
        self.text_of(self.history.get(y))
    }

    fn text_of(&self, row: Option<&Row>) -> String {
        let Some(row) = row else {
            return String::new();
        };
        let mut out = String::new();
        for cell in &row.cells {
            if cell.text.is_empty() {
                out.push(' ');
            } else {
                out.push_str(&cell.text);
            }
        }
        out.trim_end().to_string()
    }
}

impl fmt::Display for Snapshot {
    /// Renders the snapshot as the flat text form used in reports and eyeball checks.
    /// Rows that are entirely blank and unstyled collapse to `~` so a mostly-empty
    /// 24-row grid does not bury the interesting rows.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "dims {}x{} screen={:?} history={}",
            self.cols,
            self.rows,
            self.screen,
            self.history.len()
        )?;
        writeln!(
            f,
            "cursor x={} y={} pending_wrap={} visible={}",
            self.cursor.x, self.cursor.y, self.cursor.pending_wrap, self.cursor.visible
        )?;
        if !self.cursor.style.is_default() {
            writeln!(f, "cursor style {}", describe_style(&self.cursor.style))?;
        }
        for (y, row) in self.history.iter().enumerate() {
            let text = self.history_text(y);
            let flags = wrap_flags(row);
            writeln!(f, "h{y:>2} |{text}|{flags}")?;
        }
        for (y, row) in self.grid.iter().enumerate() {
            let text = self.row_text(y);
            let styled: Vec<String> = style_runs(row)
                .into_iter()
                .map(|(start, end, style)| format!("{start}..{end} {}", describe_style(&style)))
                .collect();
            if text.is_empty() && styled.is_empty() && !row.wrap && !row.wrap_continuation {
                writeln!(f, "{y:3} ~")?;
                continue;
            }
            let flags = wrap_flags(row);
            writeln!(f, "{y:3} |{text}|{flags}")?;
            for run in styled {
                writeln!(f, "    style {run}")?;
            }
        }
        Ok(())
    }
}

fn wrap_flags(row: &Row) -> &'static str {
    match (row.wrap, row.wrap_continuation) {
        (true, true) => " [wrap cont]",
        (true, false) => " [wrap]",
        (false, true) => " [cont]",
        (false, false) => "",
    }
}

/// Contiguous spans of identical non-default style within a row.
fn style_runs(row: &Row) -> Vec<(usize, usize, Style)> {
    let mut runs: Vec<(usize, usize, Style)> = Vec::new();
    for (x, cell) in row.cells.iter().enumerate() {
        if cell.style.is_default() {
            continue;
        }
        match runs.last_mut() {
            Some((_, end, style)) if *end == x && *style == cell.style => *end = x + 1,
            _ => runs.push((x, x + 1, cell.style)),
        }
    }
    runs
}

/// A one-line human description of a style, listing only what differs from default.
pub fn describe_style(style: &Style) -> String {
    let mut parts: Vec<String> = Vec::new();
    if style.fg != Color::Default {
        parts.push(format!("fg={}", describe_color(style.fg)));
    }
    if style.bg != Color::Default {
        parts.push(format!("bg={}", describe_color(style.bg)));
    }
    if style.underline_color != Color::Default {
        parts.push(format!("ul_color={}", describe_color(style.underline_color)));
    }
    for (on, name) in [
        (style.bold, "bold"),
        (style.italic, "italic"),
        (style.faint, "faint"),
        (style.blink, "blink"),
        (style.inverse, "inverse"),
        (style.invisible, "invisible"),
        (style.strikethrough, "strikethrough"),
        (style.overline, "overline"),
    ] {
        if on {
            parts.push(name.to_string());
        }
    }
    if style.underline != Underline::None {
        parts.push(format!("underline={:?}", style.underline));
    }
    if parts.is_empty() {
        "default".to_string()
    } else {
        parts.join(" ")
    }
}

pub fn describe_color(color: Color) -> String {
    match color {
        Color::Default => "default".to_string(),
        Color::Palette(i) => format!("palette({i})"),
        Color::Rgb { r, g, b } => format!("#{r:02x}{g:02x}{b:02x}"),
    }
}
