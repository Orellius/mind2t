//! The blind spot slice 6 opens: nothing could see the C ABI, because there was no C ABI.
//!
//! Every test in this project before it drives the core through its Rust API. An ABI layer
//! that mis-marshals -- a grid ref that ignores its point tag, a cell read off by one, a style
//! whose background is dropped -- is invisible to all of them, and the corpus stays 97/97
//! green while the thing a consumer would actually link is wrong.
//!
//! So the harness comes first again. This reads a `Snapshot` **entirely through the exported
//! C functions** and requires it to equal the one the core builds in Rust, over every case in
//! the real corpus. The comparison is the same `ruuah_vt_snapshot::diff` the oracle harness
//! uses, so a disagreement is located to a field rather than reported as "the grids differ".
//!
//! Transitivity is what makes this worth the machinery: the corpus already proves core ==
//! libghostty-vt, and this proves ABI == core, so ABI == libghostty-vt without ever linking
//! both libraries into one binary -- which is impossible anyway, since they define the same
//! symbols.
//!
//! Proven able to fail by `a_reader_that_ignores_the_point_tag_is_caught`, which runs the
//! identical load through a reader that asks for ACTIVE where it should ask for HISTORY.

use std::ffi::c_void;

use ruuah_vt::exports::*;
use ruuah_vt::types::*;
use ruuah_vt_snapshot::{
    Cell, Color, Cursor, Row, RowSemantic, Screen, Semantic, Snapshot, Style, Underline, Wide, diff,
};
use serde::Deserialize;

const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/cases.toml");

#[derive(Debug, Clone, Copy, Deserialize)]
struct Resize {
    cols: u16,
    rows: u16,
}

#[derive(Debug, Clone, Deserialize)]
struct Case {
    name: String,
    #[serde(default = "default_cols")]
    cols: u16,
    #[serde(default = "default_rows")]
    rows: u16,
    #[serde(default)]
    scrollback: usize,
    bytes: String,
    #[serde(default)]
    resize: Option<Resize>,
    #[serde(default)]
    after: String,
}

fn default_cols() -> u16 {
    20
}
fn default_rows() -> u16 {
    5
}

#[derive(Debug, Deserialize)]
struct Corpus {
    #[serde(rename = "case")]
    cases: Vec<Case>,
}

fn corpus() -> Vec<Case> {
    let text = std::fs::read_to_string(CORPUS).expect("the corpus is committed");
    let corpus: Corpus = toml::from_str(&text).expect("the corpus parses");
    assert!(!corpus.cases.is_empty(), "an empty corpus proves nothing");
    corpus.cases
}

/// How the reader addresses scrollback rows. The broken variant is the control.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reader {
    Correct,
    /// Wrong on purpose, and wrong in a way that still produces a VALID read: scrollback row
    /// `y` is fetched from absolute row `y + 1`. An earlier attempt used the ACTIVE tag, which
    /// only proved that an out-of-range ref is rejected -- the reader never got far enough to
    /// disagree about content. That property is worth having and is its own test below; it is
    /// not this one.
    OffByOne,
}

/// Drives a case through the C ABI and reads the result back through it too.
fn abi_snapshot(case: &Case, reader: Reader) -> Snapshot {
    unsafe {
        let mut handle: GhosttyTerminal = std::ptr::null_mut();
        let code = ghostty_terminal_new(
            std::ptr::null(),
            &mut handle,
            GhosttyTerminalOptions {
                cols: case.cols,
                rows: case.rows,
                max_scrollback: case.scrollback,
            },
        );
        assert_eq!(code, GHOSTTY_SUCCESS, "{}: terminal_new", case.name);

        let bytes = case.bytes.as_bytes();
        ghostty_terminal_vt_write(handle, bytes.as_ptr(), bytes.len());
        if let Some(resize) = case.resize {
            assert_eq!(
                ghostty_terminal_resize(handle, resize.cols, resize.rows, 10, 20),
                GHOSTTY_SUCCESS,
                "{}: resize",
                case.name
            );
        }
        let after = case.after.as_bytes();
        ghostty_terminal_vt_write(handle, after.as_ptr(), after.len());

        let snapshot = read_snapshot(handle, reader);
        ghostty_terminal_free(handle);
        snapshot
    }
}

/// Reads the whole observable state back out through the C entry points only.
unsafe fn read_snapshot(handle: GhosttyTerminal, reader: Reader) -> Snapshot {
    unsafe {
        let cols: u16 = get(handle, GHOSTTY_TERMINAL_DATA_COLS);
        let rows: u16 = get(handle, GHOSTTY_TERMINAL_DATA_ROWS);
        let scrollback: usize = get(handle, GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS);

        let screen: GhosttyTerminalScreen = get(handle, GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN);
        let mut cursor_style = GhosttyStyle {
            size: size_of::<GhosttyStyle>(),
            ..std::mem::zeroed()
        };
        assert_eq!(
            ghostty_terminal_get(
                handle,
                GHOSTTY_TERMINAL_DATA_CURSOR_STYLE,
                (&raw mut cursor_style).cast::<c_void>(),
            ),
            GHOSTTY_SUCCESS
        );

        let grid = (0..rows)
            .map(|y| read_row(handle, GHOSTTY_POINT_TAG_ACTIVE, u32::from(y), cols))
            .collect();

        let history = (0..scrollback)
            .map(|y| match reader {
                Reader::Correct => read_row(handle, GHOSTTY_POINT_TAG_HISTORY, y as u32, cols),
                // SCREEN addresses history and the active area as one sequence, so y+1 is a
                // real row rather than an error -- which is the point of this control.
                Reader::OffByOne => read_row(handle, GHOSTTY_POINT_TAG_SCREEN, y as u32 + 1, cols),
            })
            .collect();

        Snapshot {
            cols,
            rows,
            screen: match screen {
                GHOSTTY_TERMINAL_SCREEN_ALTERNATE => Screen::Alternate,
                _ => Screen::Primary,
            },
            cursor: Cursor {
                x: get(handle, GHOSTTY_TERMINAL_DATA_CURSOR_X),
                y: get(handle, GHOSTTY_TERMINAL_DATA_CURSOR_Y),
                pending_wrap: get(handle, GHOSTTY_TERMINAL_DATA_CURSOR_PENDING_WRAP),
                visible: get(handle, GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE),
                style: unpack_style(&cursor_style),
            },
            grid,
            history,
            damage: None,
        }
    }
}

unsafe fn get<T>(handle: GhosttyTerminal, data: GhosttyTerminalData) -> T {
    let mut out: T = unsafe { std::mem::zeroed() };
    let code = unsafe { ghostty_terminal_get(handle, data, (&raw mut out).cast::<c_void>()) };
    assert_eq!(code, GHOSTTY_SUCCESS, "terminal_get({data})");
    out
}

unsafe fn grid_ref(
    handle: GhosttyTerminal,
    tag: GhosttyPointTag,
    x: u16,
    y: u32,
) -> GhosttyGridRef {
    let mut out = GhosttyGridRef {
        size: size_of::<GhosttyGridRef>(),
        node: std::ptr::null_mut(),
        x: 0,
        y: 0,
    };
    let point = GhosttyPoint {
        tag,
        value: GhosttyPointValue {
            coordinate: GhosttyPointCoordinate { x, y },
        },
    };
    let code = unsafe { ghostty_terminal_grid_ref(handle, point, &mut out) };
    assert_eq!(code, GHOSTTY_SUCCESS, "grid_ref(tag={tag}, {x},{y})");
    out
}

unsafe fn read_row(handle: GhosttyTerminal, tag: GhosttyPointTag, y: u32, cols: u16) -> Row {
    unsafe {
        let first = grid_ref(handle, tag, 0, y);
        let mut raw_row: GhosttyRow = 0;
        assert_eq!(ghostty_grid_ref_row(&first, &mut raw_row), GHOSTTY_SUCCESS);

        let wrap: bool = row_get(raw_row, GHOSTTY_ROW_DATA_WRAP);
        let wrap_continuation: bool = row_get(raw_row, GHOSTTY_ROW_DATA_WRAP_CONTINUATION);
        let semantic: GhosttyRowSemanticPrompt = row_get(raw_row, GHOSTTY_ROW_DATA_SEMANTIC_PROMPT);

        Row {
            wrap,
            wrap_continuation,
            semantic_prompt: match semantic {
                GHOSTTY_ROW_SEMANTIC_PROMPT => RowSemantic::Prompt,
                GHOSTTY_ROW_SEMANTIC_PROMPT_CONTINUATION => RowSemantic::PromptContinuation,
                _ => RowSemantic::None,
            },
            cells: (0..cols).map(|x| read_cell(handle, tag, x, y)).collect(),
        }
    }
}

unsafe fn read_cell(handle: GhosttyTerminal, tag: GhosttyPointTag, x: u16, y: u32) -> Cell {
    unsafe {
        let cell_ref = grid_ref(handle, tag, x, y);

        let mut raw_cell: GhosttyCell = 0;
        assert_eq!(
            ghostty_grid_ref_cell(&cell_ref, &mut raw_cell),
            GHOSTTY_SUCCESS
        );

        let mut raw_style = GhosttyStyle {
            size: size_of::<GhosttyStyle>(),
            ..std::mem::zeroed()
        };
        assert_eq!(
            ghostty_grid_ref_style(&cell_ref, &mut raw_style),
            GHOSTTY_SUCCESS
        );

        let wide: GhosttyCellWide = cell_get(raw_cell, GHOSTTY_CELL_DATA_WIDE);
        let semantic: GhosttyCellSemanticContent =
            cell_get(raw_cell, GHOSTTY_CELL_DATA_SEMANTIC_CONTENT);

        Cell {
            text: read_graphemes(&cell_ref),
            wide: match wide {
                GHOSTTY_CELL_WIDE_WIDE => Wide::Wide,
                GHOSTTY_CELL_WIDE_SPACER_TAIL => Wide::SpacerTail,
                GHOSTTY_CELL_WIDE_SPACER_HEAD => Wide::SpacerHead,
                _ => Wide::Narrow,
            },
            style: unpack_style(&raw_style),
            semantic: match semantic {
                GHOSTTY_CELL_SEMANTIC_INPUT => Semantic::Input,
                GHOSTTY_CELL_SEMANTIC_PROMPT => Semantic::Prompt,
                _ => Semantic::Output,
            },
        }
    }
}

/// Reads a cluster, exercising the documented two-call protocol: ask with no buffer, get the
/// required length back, then ask again. A library that reported the wrong length here would
/// truncate every combining mark in a consumer that trusted it.
unsafe fn read_graphemes(cell_ref: &GhosttyGridRef) -> String {
    let mut len: usize = 0;
    let code = unsafe { ghostty_grid_ref_graphemes(cell_ref, std::ptr::null_mut(), 0, &mut len) };
    if len == 0 {
        assert_eq!(code, GHOSTTY_SUCCESS);
        return String::new();
    }
    assert_eq!(
        code, GHOSTTY_OUT_OF_SPACE,
        "a non-empty cell needs a buffer"
    );

    let mut buf = vec![0u32; len];
    assert_eq!(
        unsafe { ghostty_grid_ref_graphemes(cell_ref, buf.as_mut_ptr(), buf.len(), &mut len) },
        GHOSTTY_SUCCESS
    );
    buf[..len]
        .iter()
        .filter_map(|cp| char::from_u32(*cp))
        .collect()
}

unsafe fn cell_get<T>(cell: GhosttyCell, data: GhosttyCellData) -> T {
    let mut out: T = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { ghostty_cell_get(cell, data, (&raw mut out).cast::<c_void>()) },
        GHOSTTY_SUCCESS
    );
    out
}

unsafe fn row_get<T>(row: GhosttyRow, data: GhosttyRowData) -> T {
    let mut out: T = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { ghostty_row_get(row, data, (&raw mut out).cast::<c_void>()) },
        GHOSTTY_SUCCESS
    );
    out
}

fn unpack_style(raw: &GhosttyStyle) -> Style {
    Style {
        fg: unpack_color(&raw.fg_color),
        bg: unpack_color(&raw.bg_color),
        underline_color: unpack_color(&raw.underline_color),
        bold: raw.bold,
        italic: raw.italic,
        faint: raw.faint,
        blink: raw.blink,
        inverse: raw.inverse,
        invisible: raw.invisible,
        strikethrough: raw.strikethrough,
        overline: raw.overline,
        underline: match raw.underline {
            GHOSTTY_SGR_UNDERLINE_SINGLE => Underline::Single,
            GHOSTTY_SGR_UNDERLINE_DOUBLE => Underline::Double,
            GHOSTTY_SGR_UNDERLINE_CURLY => Underline::Curly,
            GHOSTTY_SGR_UNDERLINE_DOTTED => Underline::Dotted,
            GHOSTTY_SGR_UNDERLINE_DASHED => Underline::Dashed,
            _ => Underline::None,
        },
    }
}

fn unpack_color(raw: &GhosttyStyleColor) -> Color {
    match raw.tag {
        GHOSTTY_STYLE_COLOR_PALETTE => Color::Palette(unsafe { raw.value.palette }),
        GHOSTTY_STYLE_COLOR_RGB => {
            let rgb = unsafe { raw.value.rgb };
            Color::Rgb {
                r: rgb.r,
                g: rgb.g,
                b: rgb.b,
            }
        }
        _ => Color::Default,
    }
}

/// The same case, driven through the core's Rust API. The reference.
fn core_snapshot(case: &Case) -> Snapshot {
    let mut terminal =
        ruuah_vt_core::Terminal::with_scrollback(case.cols, case.rows, case.scrollback);
    terminal.write(case.bytes.as_bytes());
    if let Some(resize) = case.resize {
        terminal.resize(resize.cols, resize.rows);
    }
    terminal.write(case.after.as_bytes());
    terminal.snapshot()
}

#[test]
fn the_c_abi_reports_what_the_rust_api_reports() {
    let cases = corpus();
    let mut checked = 0;
    for case in &cases {
        let through_abi = abi_snapshot(case, Reader::Correct);
        let through_rust = core_snapshot(case);
        let differences = diff(&through_rust, &through_abi);
        assert!(
            differences.is_empty(),
            "case '{}' disagrees across the ABI:\n{}",
            case.name,
            differences
                .iter()
                .take(8)
                .map(|d| format!("  {d}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        checked += 1;
    }
    assert_eq!(checked, cases.len());
    assert!(checked >= 90, "the corpus shrank: only {checked} cases");
}

/// The control. A reader that fetches the wrong row is the class of bug this harness exists
/// to catch, and it must be caught -- otherwise the test above is asserting that two
/// identical code paths agree, which they trivially do.
#[test]
fn a_reader_that_fetches_the_wrong_row_is_caught() {
    let scrolled: Vec<Case> = corpus()
        .into_iter()
        .filter(|case| case.scrollback > 0)
        .collect();
    assert!(
        !scrolled.is_empty(),
        "no case retains scrollback, so this control cannot fire"
    );

    let caught = scrolled
        .iter()
        .filter(|case| {
            let broken = abi_snapshot(case, Reader::OffByOne);
            !diff(&core_snapshot(case), &broken).is_empty()
        })
        .count();

    assert!(
        caught > 0,
        "the broken reader agreed with the core on all {} scrollback cases",
        scrolled.len()
    );
}

/// A ref outside the grid is refused rather than resolved to whatever happens to be there.
/// Ghostty answers the same way, and a consumer that trusted a silently-clamped ref would
/// read the wrong cell forever without ever seeing an error.
#[test]
fn an_out_of_range_point_is_refused() {
    unsafe {
        let mut handle: GhosttyTerminal = std::ptr::null_mut();
        assert_eq!(
            ghostty_terminal_new(
                std::ptr::null(),
                &mut handle,
                GhosttyTerminalOptions {
                    cols: 4,
                    rows: 3,
                    max_scrollback: 0,
                },
            ),
            GHOSTTY_SUCCESS
        );

        let mut out = GhosttyGridRef {
            size: size_of::<GhosttyGridRef>(),
            node: std::ptr::null_mut(),
            x: 0,
            y: 0,
        };
        for (x, y) in [(0u16, 3u32), (4, 0), (99, 99)] {
            let point = GhosttyPoint {
                tag: GHOSTTY_POINT_TAG_ACTIVE,
                value: GhosttyPointValue {
                    coordinate: GhosttyPointCoordinate { x, y },
                },
            };
            assert_eq!(
                ghostty_terminal_grid_ref(handle, point, &mut out),
                GHOSTTY_INVALID_VALUE,
                "({x},{y}) is outside a 4x3 grid and must be refused"
            );
        }

        // And the in-range neighbour still resolves, so the check is not simply always-refuse.
        let point = GhosttyPoint {
            tag: GHOSTTY_POINT_TAG_ACTIVE,
            value: GhosttyPointValue {
                coordinate: GhosttyPointCoordinate { x: 3, y: 2 },
            },
        };
        assert_eq!(
            ghostty_terminal_grid_ref(handle, point, &mut out),
            GHOSTTY_SUCCESS
        );

        ghostty_terminal_free(handle);
    }
}

/// Parity on the failure path, which the corpus cannot reach.
///
/// Every case in this file drives a terminal that was created successfully; nothing here
/// otherwise observes what happens when creation fails. The oracle's behaviour is measured in
/// `ruuah-vt-ghostty`'s `a_failed_creation_nulls_the_out_param`, and this is the same
/// assertion against our export, so the two move together or the test says so.
#[test]
fn a_failed_creation_nulls_the_out_param_here_too() {
    let sentinel = 0xDEAD_BEEF_usize as GhosttyTerminal;
    let mut handle = sentinel;
    let options = GhosttyTerminalOptions {
        cols: 0,
        rows: 24,
        max_scrollback: 0,
    };

    let code = unsafe { ghostty_terminal_new(std::ptr::null(), &mut handle, options) };

    assert_eq!(code, GHOSTTY_INVALID_VALUE);
    assert!(
        handle.is_null(),
        "left {handle:?} in the out-param; a stale non-null handle reads as success"
    );
}

/// The style id is the one cell field with no consumer inside this harness: `read_cell` gets
/// the style by grid ref, so a `STYLE_ID` that is always zero passes every test above. The
/// audit's mutation M7 proved exactly that -- returning `0xDEAD` from the branch changed
/// nothing. This asserts the property a consumer actually gates on: a default cell is 0 and a
/// styled cell is not, and two cells wearing the same style report the same id.
#[test]
fn a_styled_cell_reports_a_non_zero_style_id() {
    unsafe {
        let mut handle: GhosttyTerminal = std::ptr::null_mut();
        assert_eq!(
            ghostty_terminal_new(
                std::ptr::null(),
                &mut handle,
                GhosttyTerminalOptions {
                    cols: 8,
                    rows: 2,
                    max_scrollback: 0,
                },
            ),
            GHOSTTY_SUCCESS
        );

        // "a" plain, then "bc" bold: one unstyled cell and two sharing one style.
        let bytes = b"a\x1b[1mbc";
        ghostty_terminal_vt_write(handle, bytes.as_ptr(), bytes.len());

        let id_of = |x: u16| -> u16 {
            let cell_ref = grid_ref(handle, GHOSTTY_POINT_TAG_ACTIVE, x, 0);
            let mut raw: GhosttyCell = 0;
            assert_eq!(
                ghostty_grid_ref_cell(&cell_ref, &mut raw),
                GHOSTTY_SUCCESS
            );
            cell_get::<u16>(raw, GHOSTTY_CELL_DATA_STYLE_ID)
        };

        let plain = id_of(0);
        let bold_b = id_of(1);
        let bold_c = id_of(2);

        assert_eq!(plain, 0, "an unstyled cell must report the default style id");
        assert_ne!(
            bold_b, 0,
            "a bold cell reported style id 0, which is the default style -- a consumer \
             gating on the id sees every cell as unstyled"
        );
        assert_eq!(
            bold_b, bold_c,
            "two cells wearing the same style must report the same id"
        );

        ghostty_terminal_free(handle);
    }
}

/// The content tag is the other cell field with no consumer inside this harness: `read_cell`
/// gets the cluster through `read_graphemes`, so a `CONTENT_TAG` frozen at `CODEPOINT` passes
/// the whole corpus -- which is exactly what the audit's finding 13 found shipping. The
/// oracle's rule is measured in `ruuah-vt-ghostty`'s
/// `a_cell_holding_a_multi_codepoint_cluster_wears_the_grapheme_content_tag`: more than one
/// codepoint in the cell means `CODEPOINT_GRAPHEME`. A consumer gating its graphemes call on
/// the tag drops every niqqud otherwise.
#[test]
fn a_grapheme_cell_reports_the_grapheme_content_tag() {
    unsafe {
        let mut handle: GhosttyTerminal = std::ptr::null_mut();
        assert_eq!(
            ghostty_terminal_new(
                std::ptr::null(),
                &mut handle,
                GhosttyTerminalOptions {
                    cols: 8,
                    rows: 2,
                    max_scrollback: 0,
                },
            ),
            GHOSTTY_SUCCESS
        );

        // 'A' at column 0; bet + dagesh (U+05D1 U+05BC) as one two-codepoint cell at column 1.
        let bytes = "A\u{05D1}\u{05BC}".as_bytes();
        ghostty_terminal_vt_write(handle, bytes.as_ptr(), bytes.len());

        let tag_of = |x: u16| -> GhosttyCellContentTag {
            let cell_ref = grid_ref(handle, GHOSTTY_POINT_TAG_ACTIVE, x, 0);
            let mut raw: GhosttyCell = 0;
            assert_eq!(ghostty_grid_ref_cell(&cell_ref, &mut raw), GHOSTTY_SUCCESS);
            cell_get::<GhosttyCellContentTag>(raw, GHOSTTY_CELL_DATA_CONTENT_TAG)
        };

        assert_eq!(
            tag_of(1),
            GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME,
            "a base-plus-combining-mark cell must wear the grapheme tag, as the oracle does"
        );
        assert_eq!(
            tag_of(0),
            GHOSTTY_CELL_CONTENT_CODEPOINT,
            "a single-codepoint cell must stay a plain codepoint cell"
        );

        ghostty_terminal_free(handle);
    }
}
