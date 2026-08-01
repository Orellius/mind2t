//! Purpose: the terminal state a differential test is allowed to observe.
//! Public surface: `Snapshot`, `Row`, `Cell`, `Cursor`, `Style`, `Color`, `Wide`,
//!   `Underline`, `Screen`, `Dirty`, `Damage`, `Semantic`, `RowSemantic`, and a `Display`
//!   impl that renders a snapshot for human eyes.
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

/// What a cell's content means, as marked by the OSC 133 semantic prompt sequences.
///
/// `Output` is not "unmarked": a terminal that has never seen an OSC 133 reports every cell
/// as output, which is what the sequences themselves define as the default state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Semantic {
    /// Command output, and the state everything starts in.
    #[default]
    Output,
    /// Typed by the user at a prompt.
    Input,
    /// Emitted by the shell as the prompt itself.
    Prompt,
}

/// Whether a row takes part in a shell prompt.
///
/// A row-level summary that exists so jump-to-prompt does not have to walk cells. The ABI
/// documents it as allowing false positives but never false negatives, so it is a coarser
/// signal than the per-cell `Semantic` rather than a derived one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RowSemantic {
    /// No prompt cells in this row.
    #[default]
    None,
    /// Prompt cells exist and this row begins a prompt.
    Prompt,
    /// Prompt cells exist and this row continues a prompt started above.
    PromptContinuation,
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
    /// What OSC 133 says this cell's content is. Blind before slice 5.6: nothing produced
    /// anything but `Output`, so a core with no OSC 133 at all reported a perfect match.
    pub semantic: Semantic,
}

/// One grid row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// This row soft-wraps into the next one.
    pub wrap: bool,
    /// This row is the continuation of a soft-wrap from the previous one.
    pub wrap_continuation: bool,
    /// The row's own prompt state, which is tracked separately from its cells rather than
    /// summarised from them.
    pub semantic_prompt: RowSemantic,
    pub cells: Vec<Cell>,
}

/// How much of the screen changed since the dirty flags were last reset.
///
/// Two independent layers, matching the ABI: a global state saying whether the frame is
/// clean, partly dirty, or wholly dirty, and a per-row flag for the partial case. Setting one
/// does not clear the other, which is the trap the header calls out explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dirty {
    /// Nothing changed; a renderer can skip the frame entirely.
    None,
    /// Some rows changed. Consult the per-row flags.
    Partial,
    /// Everything changed; redraw without consulting rows.
    Full,
}

/// What a renderer would have to repaint.
///
/// Optional on a `Snapshot` because most corpus cases do not ask for it: damage is
/// accumulated between two writes, so a case has to opt in to be meaningful at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Damage {
    pub global: Dirty,
    /// One flag per active row, top to bottom.
    pub rows: Vec<bool>,
}

/// Terminal modes that leave no trace on the grid.
///
/// A mode like bracketed paste changes only what the HOST writes to the pty, so a core
/// that ignores it entirely scores a perfect match on every grid comparison — which is
/// exactly the blind-spot shape every slice has hit. These are compared as state, not
/// as behaviour, because there is no behaviour inside the core to compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modes {
    /// DEC private mode 2004: the host wraps pastes in `ESC[200~` / `ESC[201~`.
    pub bracketed_paste: bool,
    /// DEC private mode 2026: the pump holds frames back until the batch closes.
    pub synchronized_output: bool,
    /// DEC private modes 9/1000/1002/1003: which pointer events the child asked for.
    /// These are the RAW bits as the oracle's mode table stores them (`modes.zig`) --
    /// what `ghostty_terminal_mode_get` answers from -- not the derived last-writer
    /// event kind, which the ABI does not expose. After `1000h 1002h 1002l` the 1000
    /// bit is still set even though reporting is off; comparing raw bits is what lets
    /// a differential see that distinction at all.
    pub mouse_event_x10: bool,
    pub mouse_event_normal: bool,
    pub mouse_event_button: bool,
    pub mouse_event_any: bool,
    /// DEC private modes 1005/1006/1015/1016: the report encoding the child asked for.
    pub mouse_format_utf8: bool,
    pub mouse_format_sgr: bool,
    pub mouse_format_urxvt: bool,
    pub mouse_format_sgr_pixels: bool,
    /// DEC private mode 1007: wheel-to-arrows on the alternate screen. The one tracked
    /// mode that DEFAULTS ON (the oracle marks it `.default = true`), which is why
    /// `Default` here is hand-written.
    pub mouse_alternate_scroll: bool,
    /// DEC private mode 1 (DECCKM): application cursor keys. Tracked because the
    /// alternate-scroll wheel path picks `ESC O A` over `ESC [ A` by it.
    pub cursor_keys: bool,
    /// DEC private mode 66 (DECKPAM/DECKPNM, also ESC = / ESC >): keypad application.
    pub keypad_keys: bool,
    /// DEC private mode 1035 (default ON): numlock suppresses keypad application.
    pub ignore_keypad_with_numlock: bool,
    /// DEC private mode 1036 (default ON): alt prefixes ESC in legacy key encoding.
    pub alt_esc_prefix: bool,
}

impl Default for Modes {
    fn default() -> Self {
        Self {
            bracketed_paste: false,
            synchronized_output: false,
            mouse_event_x10: false,
            mouse_event_normal: false,
            mouse_event_button: false,
            mouse_event_any: false,
            mouse_format_utf8: false,
            mouse_format_sgr: false,
            mouse_format_urxvt: false,
            mouse_format_sgr_pixels: false,
            mouse_alternate_scroll: true,
            cursor_keys: false,
            keypad_keys: false,
            ignore_keypad_with_numlock: true,
            alt_esc_prefix: true,
        }
    }
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

/// One colour as the terminal stores it: 8 bits per channel, exactly the oracle's
/// `GhosttyColorRgb`. OSC queries report 16-bit channels, but that is a REPLY encoding
/// (`v * 0x101`), not stored precision -- storing 16 bits here would let the two sides
/// agree on every byte while disagreeing about what they would report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// The OSC-addressable colour state (OSC 4/104 indexed, 10/11/12 + 110/111/112 dynamic).
///
/// Like `Modes`, none of this leaves a trace on the grid: a core that ignores OSC 4
/// entirely scores a perfect match on every cell comparison, because cells carry palette
/// INDICES and the table those indices resolve through lives here. The oracle exposes it
/// through `GHOSTTY_TERMINAL_DATA_COLOR_*`, which is what makes this a real differential
/// observable rather than a source-referenced promise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Colors {
    /// Effective foreground: the OSC 10 override, else the embedder default, else `None`.
    /// `None` is the fresh-terminal state -- the VT core has no opinion about the
    /// renderer's default ink, and the oracle answers `GHOSTTY_NO_VALUE` there.
    pub foreground: Option<Rgb>,
    /// Effective background (OSC 11 override, else embedder default, else `None`).
    pub background: Option<Rgb>,
    /// Effective cursor colour (OSC 12 override, else embedder default, else `None`).
    pub cursor: Option<Rgb>,
    /// The current 256-entry palette, OSC 4 overrides applied. Always present: unlike
    /// the dynamic colours the palette has a built-in default, and both implementations
    /// must agree on it entry for entry or every case diffs here.
    pub palette: Vec<Rgb>,
}

impl Default for Colors {
    fn default() -> Self {
        Colors {
            foreground: None,
            background: None,
            cursor: None,
            palette: default_palette(),
        }
    }
}

/// The oracle's built-in palette, measured from its source 2026-08-01
/// (`color.zig`, `pub const default`): indices 0-15 are Ghostty's own named
/// defaults (the Tomorrow scheme, NOT classic xterm's CD0000 family), 16-231 the
/// standard 6x6x6 cube (`0` or `n*40+55` per channel), 232-255 the standard gray
/// ramp (`(i-232)*10+8`). Both implementations initialize from this one table;
/// the differential compares every entry, so a drift in either copy surfaces as
/// `colors.palette[i]` on every case.
pub fn default_palette() -> Vec<Rgb> {
    const NAMED: [(u8, u8, u8); 16] = [
        (0x1D, 0x1F, 0x21),
        (0xCC, 0x66, 0x66),
        (0xB5, 0xBD, 0x68),
        (0xF0, 0xC6, 0x74),
        (0x81, 0xA2, 0xBE),
        (0xB2, 0x94, 0xBB),
        (0x8A, 0xBE, 0xB7),
        (0xC5, 0xC8, 0xC6),
        (0x66, 0x66, 0x66),
        (0xD5, 0x4E, 0x53),
        (0xB9, 0xCA, 0x4A),
        (0xE7, 0xC5, 0x47),
        (0x7A, 0xA6, 0xDA),
        (0xC3, 0x97, 0xD8),
        (0x70, 0xC0, 0xB1),
        (0xEA, 0xEA, 0xEA),
    ];
    let mut palette = Vec::with_capacity(256);
    for (r, g, b) in NAMED {
        palette.push(Rgb { r, g, b });
    }
    let scale = |n: u8| if n == 0 { 0 } else { n * 40 + 55 };
    for r in 0..6 {
        for g in 0..6 {
            for b in 0..6 {
                palette.push(Rgb { r: scale(r), g: scale(g), b: scale(b) });
            }
        }
    }
    for i in 0..24u8 {
        let value = i * 10 + 8;
        palette.push(Rgb { r: value, g: value, b: value });
    }
    palette
}

/// The full observable state of a terminal at one instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub cols: u16,
    pub rows: u16,
    pub screen: Screen,
    pub cursor: Cursor,
    /// Modes with no grid-observable effect of their own (see `Modes`).
    pub modes: Modes,
    /// Active-area rows, top to bottom. Length is `rows`; each row holds `cols` cells.
    pub grid: Vec<Row>,
    /// Scrollback rows above the active area, oldest first.
    ///
    /// Separate from `grid` rather than prepended to it so the active area keeps stable
    /// coordinates: `cell[0,0]` means the same thing whether or not history exists, which
    /// keeps every pre-scrollback corpus case and its recorded diffs valid.
    pub history: Vec<Row>,
    /// What changed since the case reset the dirty flags, when the case asked to observe it.
    /// `None` on both sides for every case that does not, which is how the pre-slice-5
    /// corpus stays untouched.
    pub damage: Option<Damage>,
    /// The working directory the child last reported (OSC 7), stored as the raw bytes it
    /// emitted -- typically a `file://` URI, never parsed or decoded.
    ///
    /// Empty is the only "unset": the oracle's `setPwd("")` clears the buffer, so a cleared
    /// pwd and one never set are the same observable state. Bytes rather than a `String`
    /// because a path need not be UTF-8 and lossy decoding would make two different
    /// payloads compare equal.
    pub pwd: Vec<u8>,
    /// The OSC-addressable colour state (see `Colors`).
    pub colors: Colors,
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
            semantic: Semantic::Output,
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
        if let Some(damage) = &self.damage {
            let rows: String = damage
                .rows
                .iter()
                .map(|dirty| if *dirty { '#' } else { '.' })
                .collect();
            writeln!(f, "damage {:?} rows |{rows}|", damage.global)?;
        }
        // History rows render through the same path as active ones. They used not to, and
        // the omission was invisible until slice 5.6 read a second per-cell layer: a style
        // or semantic difference in scrollback was reported by `diff` and then absent from
        // the dump a human reads to diagnose it.
        for (y, row) in self.history.iter().enumerate() {
            write_row(f, &format!("h{y:>2}"), &self.history_text(y), row, false)?;
        }
        for (y, row) in self.grid.iter().enumerate() {
            write_row(f, &format!("{y:3}"), &self.row_text(y), row, true)?;
        }
        Ok(())
    }
}

/// Renders one row and its per-cell layers under a caller-supplied label.
///
/// `collapse` writes an entirely empty row as `~`, which keeps a mostly-blank 24-row active
/// area readable. History is never collapsed: every row in it was written on purpose.
fn write_row(
    f: &mut fmt::Formatter<'_>,
    label: &str,
    text: &str,
    row: &Row,
    collapse: bool,
) -> fmt::Result {
    let styled: Vec<String> = style_runs(row)
        .into_iter()
        .map(|(start, end, style)| format!("{start}..{end} {}", describe_style(&style)))
        .collect();
    let semantic: Vec<String> = semantic_runs(row)
        .into_iter()
        .map(|(start, end, semantic)| format!("{start}..{end} {semantic:?}"))
        .collect();

    if collapse
        && text.is_empty()
        && styled.is_empty()
        && semantic.is_empty()
        && !row.wrap
        && !row.wrap_continuation
        && row.semantic_prompt == RowSemantic::None
    {
        return writeln!(f, "{label} ~");
    }

    let prompt = match row.semantic_prompt {
        RowSemantic::None => "",
        RowSemantic::Prompt => " [prompt]",
        RowSemantic::PromptContinuation => " [prompt cont]",
    };
    writeln!(f, "{label} |{text}|{}{prompt}", wrap_flags(row))?;
    for run in styled {
        writeln!(f, "    style {run}")?;
    }
    for run in semantic {
        writeln!(f, "    semantic {run}")?;
    }
    Ok(())
}

fn wrap_flags(row: &Row) -> &'static str {
    match (row.wrap, row.wrap_continuation) {
        (true, true) => " [wrap cont]",
        (true, false) => " [wrap]",
        (false, true) => " [cont]",
        (false, false) => "",
    }
}

/// Contiguous spans of non-`Output` semantic content within a row.
fn semantic_runs(row: &Row) -> Vec<(usize, usize, Semantic)> {
    let mut runs: Vec<(usize, usize, Semantic)> = Vec::new();
    for (x, cell) in row.cells.iter().enumerate() {
        if cell.semantic == Semantic::Output {
            continue;
        }
        match runs.last_mut() {
            Some((_, end, semantic)) if *end == x && *semantic == cell.semantic => *end = x + 1,
            _ => runs.push((x, x + 1, cell.semantic)),
        }
    }
    runs
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

/// Renders a raw byte payload for a difference report: readable when it is text, and
/// unambiguous when it is not.
///
/// A pwd is usually a URI and usually short, but it is never guaranteed to be either, so a
/// long value is elided in the middle -- the head and tail are what identify it, and a
/// 4096-byte difference message helps nobody.
pub fn describe_bytes(bytes: &[u8]) -> String {
    const HEAD: usize = 48;
    const TAIL: usize = 16;

    if bytes.is_empty() {
        return "<empty>".to_string();
    }
    let render = |slice: &[u8]| {
        slice
            .iter()
            .flat_map(|&b| std::ascii::escape_default(b))
            .map(char::from)
            .collect::<String>()
    };
    if bytes.len() <= HEAD + TAIL {
        return format!("\"{}\"", render(bytes));
    }
    format!(
        "\"{}\"..{} more..\"{}\"",
        render(&bytes[..HEAD]),
        bytes.len() - HEAD - TAIL,
        render(&bytes[bytes.len() - TAIL..]),
    )
}

pub fn describe_rgb(color: Option<Rgb>) -> String {
    match color {
        None => "<unset>".to_string(),
        Some(Rgb { r, g, b }) => format!("#{r:02x}{g:02x}{b:02x}"),
    }
}

pub fn describe_color(color: Color) -> String {
    match color {
        Color::Default => "default".to_string(),
        Color::Palette(i) => format!("palette({i})"),
        Color::Rgb { r, g, b } => format!("#{r:02x}{g:02x}{b:02x}"),
    }
}
