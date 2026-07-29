//! Purpose: prove the oracle reads a real grid correctly, before anything is compared to it.
//! Public surface: none, this is a test.
//! Why this file: `abi_layout.rs` proves the struct offsets are right; this proves the
//!   readout built on them is right. Both are needed: correct offsets read in the wrong
//!   order still produce a confident wrong answer.
//! NOT responsible for: ruuah-vt's own behaviour, or any comparison between the two.
//! Test strategy: drive libghostty-vt with byte streams whose correct outcome is fixed by
//!   the VT spec, and assert on the snapshot rather than on internal state.

use ruuah_vt_ghostty::Terminal;
use ruuah_vt_snapshot::{Color, Screen, Style, Underline, Wide};

fn snapshot_of(cols: u16, rows: u16, bytes: &[u8]) -> ruuah_vt_snapshot::Snapshot {
    let mut terminal = Terminal::new(cols, rows).expect("terminal creation");
    terminal.write(bytes);
    terminal.snapshot().expect("snapshot")
}

#[test]
fn a_fresh_terminal_is_blank_with_the_cursor_at_the_origin() {
    let snap = snapshot_of(20, 5, b"");

    assert_eq!((snap.cols, snap.rows), (20, 5));
    assert_eq!(snap.screen, Screen::Primary);
    assert_eq!((snap.cursor.x, snap.cursor.y), (0, 0));
    assert!(!snap.cursor.pending_wrap);
    assert_eq!(snap.grid.len(), 5);
    assert!(snap.grid.iter().all(|row| row.cells.len() == 20));
    assert!(
        snap.grid
            .iter()
            .flat_map(|row| &row.cells)
            .all(|cell| cell.text.is_empty() && cell.style.is_default())
    );
}

#[test]
fn printable_text_lands_in_cells_and_advances_the_cursor() {
    let snap = snapshot_of(20, 3, b"hello");

    assert_eq!(snap.row_text(0), "hello");
    assert_eq!(snap.grid[0].cells[0].text, "h");
    assert_eq!(snap.grid[0].cells[4].text, "o");
    assert!(snap.grid[0].cells[5].text.is_empty());
    assert_eq!((snap.cursor.x, snap.cursor.y), (5, 0));
}

#[test]
fn carriage_return_and_line_feed_move_the_cursor_without_erasing() {
    let snap = snapshot_of(20, 3, b"one\r\ntwo");

    assert_eq!(snap.row_text(0), "one");
    assert_eq!(snap.row_text(1), "two");
    assert_eq!((snap.cursor.x, snap.cursor.y), (3, 1));
}

#[test]
fn sgr_bold_applies_to_the_cells_written_while_it_is_active() {
    let snap = snapshot_of(20, 2, b"a\x1b[1mB\x1b[0mc");

    assert_eq!(snap.row_text(0), "aBc");
    assert!(!snap.grid[0].cells[0].style.bold, "plain before SGR");
    assert!(snap.grid[0].cells[1].style.bold, "bold while SGR 1 active");
    assert!(!snap.grid[0].cells[2].style.bold, "reset by SGR 0");
}

#[test]
fn sgr_colours_are_read_back_as_palette_and_rgb() {
    // SGR 31 is palette 1; SGR 38;2;R;G;B is direct RGB.
    let snap = snapshot_of(20, 2, b"\x1b[31mp\x1b[38;2;10;20;30mr");

    assert_eq!(snap.grid[0].cells[0].style.fg, Color::Palette(1));
    assert_eq!(
        snap.grid[0].cells[1].style.fg,
        Color::Rgb {
            r: 10,
            g: 20,
            b: 30
        }
    );
}

#[test]
fn sgr_curly_underline_survives_the_conversion() {
    let snap = snapshot_of(20, 2, b"\x1b[4:3mx");

    assert_eq!(snap.grid[0].cells[0].style.underline, Underline::Curly);
}

#[test]
fn cursor_position_sequences_are_reflected() {
    // CUP moves to row 3, column 5 (both 1-indexed in the sequence).
    let snap = snapshot_of(20, 10, b"\x1b[3;5H");

    assert_eq!((snap.cursor.x, snap.cursor.y), (4, 2));
}

#[test]
fn writing_the_last_column_sets_pending_wrap_without_moving_the_cursor() {
    // The DEC phantom state: the cursor stays on the last column and the wrap is deferred
    // until the next printable character. This is failure mode 5 in the plan and the
    // oracle must expose it, or slice 2 has nothing to test against.
    let snap = snapshot_of(5, 3, b"abcde");

    assert_eq!(snap.cursor.x, 4, "cursor stays on the last column");
    assert_eq!(snap.cursor.y, 0, "and has not moved down yet");
    assert!(snap.cursor.pending_wrap, "the wrap is pending");

    let after = snapshot_of(5, 3, b"abcdef");
    assert_eq!((after.cursor.x, after.cursor.y), (1, 1), "next print wraps");
    assert!(!after.cursor.pending_wrap);
    assert!(after.grid[0].wrap, "row 0 is marked soft-wrapped");
    assert!(after.grid[1].wrap_continuation, "row 1 is its continuation");
}

#[test]
fn a_wide_character_occupies_two_cells_with_a_spacer_tail() {
    // A cell is not a codepoint. Failure mode 2 in the plan; the snapshot must carry it.
    let snap = snapshot_of(10, 2, "\u{4e16}x".as_bytes());

    assert_eq!(snap.grid[0].cells[0].text, "\u{4e16}");
    assert_eq!(snap.grid[0].cells[0].wide, Wide::Wide);
    assert_eq!(snap.grid[0].cells[1].wide, Wide::SpacerTail);
    assert_eq!(snap.grid[0].cells[2].text, "x");
    assert_eq!(snap.cursor.x, 3, "the wide cell advanced the cursor by two");
}

#[test]
fn a_combining_mark_stays_in_one_cell_as_one_cluster() {
    // e + U+0301 combining acute is a single grapheme cluster in a single cell.
    let snap = snapshot_of(10, 2, "e\u{301}!".as_bytes());

    assert_eq!(snap.grid[0].cells[0].text, "e\u{301}");
    assert_eq!(snap.grid[0].cells[1].text, "!");
    assert_eq!(snap.cursor.x, 2);
}

#[test]
fn the_alternate_screen_is_reported_as_such() {
    let snap = snapshot_of(10, 3, b"\x1b[?1049h");

    assert_eq!(snap.screen, Screen::Alternate);
}

#[test]
fn a_space_is_distinguishable_from_an_untouched_cell() {
    // The differ relies on this: erasing to a space is not the same state as never
    // having written. If the oracle collapsed them, whole classes of erase bugs would
    // be invisible to the harness.
    let snap = snapshot_of(5, 2, b"a b");

    assert_eq!(snap.grid[0].cells[1].text, " ", "explicit space");
    assert_eq!(snap.grid[0].cells[3].text, "", "never written");
}

#[test]
fn the_cursor_carries_the_style_that_the_next_write_will_use() {
    let snap = snapshot_of(10, 2, b"\x1b[1;3m");

    assert!(snap.cursor.style.bold);
    assert!(snap.cursor.style.italic);
    assert_ne!(snap.cursor.style, Style::DEFAULT);
}

#[test]
fn a_terminal_can_be_written_in_several_chunks() {
    // The corpus feeds whole streams, but a real pty arrives in fragments and the parser
    // must be resumable across a write boundary mid-sequence.
    let mut whole = Terminal::new(10, 2).unwrap();
    whole.write(b"\x1b[1mbold");

    let mut split = Terminal::new(10, 2).unwrap();
    split.write(b"\x1b[1");
    split.write(b"mbo");
    split.write(b"ld");

    assert_eq!(whole.snapshot().unwrap(), split.snapshot().unwrap());
}

#[test]
fn a_background_only_cell_reports_its_colour_in_the_style() {
    // Ghostty keeps a cell that has only a background out of the style map, using a
    // bg_color content tag instead. Reading the style alone reports Default and the harness
    // would be blind to background-colour erase -- an erase that ignored BCE would pass.
    // Measured 2026-07-28; this pins the normalisation that closes it.
    let snap = snapshot_of(8, 2, b"abc\x1b[41m\x1b[2K");

    assert_eq!(snap.grid[0].cells[0].text, "", "the line was erased");
    assert_eq!(
        snap.grid[0].cells[0].style.bg,
        Color::Palette(1),
        "erased cells must carry the background that erased them"
    );
}

#[test]
fn background_only_and_printed_space_are_both_visible_but_distinguishable() {
    let erased = snapshot_of(8, 2, b"\x1b[41m\x1b[2K");
    let printed = snapshot_of(8, 2, b"\x1b[41m ");

    assert_eq!(erased.grid[0].cells[0].style.bg, Color::Palette(1));
    assert_eq!(printed.grid[0].cells[0].style.bg, Color::Palette(1));
    assert_eq!(erased.grid[0].cells[0].text, "", "no text");
    assert_eq!(printed.grid[0].cells[0].text, " ", "a real space");
}

#[test]
fn a_direct_rgb_background_survives_erase() {
    let snap = snapshot_of(8, 2, b"\x1b[48;2;10;20;30m\x1b[2J");

    assert_eq!(
        snap.grid[1].cells[3].style.bg,
        Color::Rgb {
            r: 10,
            g: 20,
            b: 30
        }
    );
}

/// Damage is the slice-5 blind spot, so this pins that the oracle reports it AND that a
/// reset clears it. Without both halves the harness could not tell a correct damage
/// implementation from one that marks everything dirty forever.
#[test]
fn the_oracle_reports_damage_and_a_reset_clears_it() {
    let mut terminal = Terminal::new(10, 3).expect("terminal");
    let mut render = ruuah_vt_ghostty::RenderState::new().expect("render state");

    terminal.write(b"hello");
    render.update(&terminal).expect("update");
    let after_write = render.damage().expect("damage");
    assert_ne!(
        after_write.global,
        ruuah_vt_snapshot::Dirty::None,
        "a write must dirty the frame, or the harness is blind"
    );

    render.clear_dirty().expect("clear");
    render.update(&terminal).expect("update");
    let after_reset = render.damage().expect("damage");
    assert_eq!(
        after_reset.global,
        ruuah_vt_snapshot::Dirty::None,
        "a reset with no further writes must leave the frame clean"
    );
    assert!(
        after_reset.rows.iter().all(|dirty| !dirty),
        "the reset must clear the per-row layer too, not just the global one: {:?}",
        after_reset.rows
    );

    terminal.write(b"\r\nsecond");
    render.update(&terminal).expect("update");
    let targeted = render.damage().expect("damage");
    assert_ne!(targeted.global, ruuah_vt_snapshot::Dirty::None);
    assert!(
        targeted.rows.iter().any(|dirty| *dirty),
        "writing one row must dirty at least that row: {:?}",
        targeted.rows
    );
    assert!(
        !targeted.rows.iter().all(|dirty| *dirty),
        "and must NOT dirty every row, or per-row damage carries no information: {:?}",
        targeted.rows
    );
}

/// What the oracle does with the out-param when creation fails.
///
/// Measured rather than read, because the drop-in claim rests on it: a C consumer that checks
/// the out-param instead of the return code sees whatever was in that variable before the
/// call, and the two libraries have to agree about what that is.
#[test]
fn a_failed_creation_nulls_the_out_param() {
    let sentinel = 0xDEAD_BEEF_usize as ruuah_vt_ghostty::sys::GhosttyTerminal;
    let mut handle = sentinel;
    let options = ruuah_vt_ghostty::sys::GhosttyTerminalOptions {
        cols: 0,
        rows: 24,
        max_scrollback: 0,
    };

    let code = unsafe {
        ruuah_vt_ghostty::sys::ghostty_terminal_new(std::ptr::null(), &mut handle, options)
    };

    assert_ne!(
        code,
        ruuah_vt_ghostty::sys::GhosttyResult_GHOSTTY_SUCCESS,
        "zero columns must fail"
    );
    assert!(
        handle.is_null(),
        "the oracle left {handle:?} in the out-param on failure; the sentinel was {sentinel:?}"
    );
}

/// A pure out-param's `.size` is written by the oracle, never read: every one of these is a
/// whole-struct assign in the source (`style.zig` `default_style`, `grid_ref.zig` `fromPin` /
/// `grid_ref_style`, `terminal.zig` `.cursor_style`), with `size` coming from the struct's
/// own `@sizeOf` default -- and upstream's own tests pass `undefined` out-params, which is
/// sound only because nothing reads them. `GHOSTTY_INIT_SIZED` governs structs passed IN
/// (`style_is_default` does assert the caller's `.size`), not these. Measured with garbage
/// in the field, because a consumer following upstream's own test pattern sends exactly that.
#[test]
fn a_pure_out_params_size_is_overwritten_not_echoed() {
    use ruuah_vt_ghostty::sys;
    const GARBAGE: usize = 0xDEAD;
    unsafe {
        let mut handle: sys::GhosttyTerminal = std::ptr::null_mut();
        let options = sys::GhosttyTerminalOptions {
            cols: 20,
            rows: 3,
            max_scrollback: 0,
        };
        assert_eq!(
            sys::ghostty_terminal_new(std::ptr::null(), &mut handle, options),
            sys::GhosttyResult_GHOSTTY_SUCCESS
        );
        let bytes = b"\x1b[1mB";
        sys::ghostty_terminal_vt_write(handle, bytes.as_ptr(), bytes.len());

        // ghostty_style_default
        let mut style: sys::GhosttyStyle = std::mem::zeroed();
        style.size = GARBAGE;
        sys::ghostty_style_default(&mut style);
        assert_eq!(style.size, size_of::<sys::GhosttyStyle>());

        // ghostty_terminal_get(CURSOR_STYLE)
        let mut cursor_style: sys::GhosttyStyle = std::mem::zeroed();
        cursor_style.size = GARBAGE;
        assert_eq!(
            sys::ghostty_terminal_get(
                handle,
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_STYLE,
                (&raw mut cursor_style).cast(),
            ),
            sys::GhosttyResult_GHOSTTY_SUCCESS
        );
        assert_eq!(cursor_style.size, size_of::<sys::GhosttyStyle>());

        // ghostty_terminal_grid_ref
        let point = sys::GhosttyPoint {
            tag: sys::GhosttyPointTag_GHOSTTY_POINT_TAG_ACTIVE,
            value: sys::GhosttyPointValue {
                coordinate: sys::GhosttyPointCoordinate { x: 0, y: 0 },
            },
        };
        let mut cell_ref: sys::GhosttyGridRef = std::mem::zeroed();
        cell_ref.size = GARBAGE;
        assert_eq!(
            sys::ghostty_terminal_grid_ref(handle, point, &mut cell_ref),
            sys::GhosttyResult_GHOSTTY_SUCCESS
        );
        assert_eq!(cell_ref.size, size_of::<sys::GhosttyGridRef>());

        // ghostty_grid_ref_style
        let mut cell_style: sys::GhosttyStyle = std::mem::zeroed();
        cell_style.size = GARBAGE;
        assert_eq!(
            sys::ghostty_grid_ref_style(&cell_ref, &mut cell_style),
            sys::GhosttyResult_GHOSTTY_SUCCESS
        );
        assert_eq!(cell_style.size, size_of::<sys::GhosttyStyle>());

        sys::ghostty_terminal_free(handle);
    }
}

/// The oracle's content-tag rule for grapheme clusters, measured through its own C ABI.
///
/// Upstream flips a cell's tag to `codepoint_grapheme` the moment a continuation codepoint is
/// attached (`page.zig`, `appendGrapheme`), and `hasGrapheme` is defined as exactly that tag --
/// so the rule is: more than one codepoint in the cell means the grapheme tag, one means plain
/// `codepoint`. This pins it, because our `ghostty_cell_get` must say the same thing and the
/// snapshot readout above never touches the tag (it reads clusters via the graphemes call).
#[test]
fn a_cell_holding_a_multi_codepoint_cluster_wears_the_grapheme_content_tag() {
    use ruuah_vt_ghostty::sys;
    unsafe {
        let mut handle: sys::GhosttyTerminal = std::ptr::null_mut();
        let options = sys::GhosttyTerminalOptions {
            cols: 20,
            rows: 3,
            max_scrollback: 0,
        };
        assert_eq!(
            sys::ghostty_terminal_new(std::ptr::null(), &mut handle, options),
            sys::GhosttyResult_GHOSTTY_SUCCESS
        );

        // 'A' at column 0; bet + dagesh (U+05D1 U+05BC) as one two-codepoint cell at column 1.
        let bytes = "A\u{05D1}\u{05BC}".as_bytes();
        sys::ghostty_terminal_vt_write(handle, bytes.as_ptr(), bytes.len());

        let tag_of = |x: u16| -> sys::GhosttyCellContentTag {
            let point = sys::GhosttyPoint {
                tag: sys::GhosttyPointTag_GHOSTTY_POINT_TAG_ACTIVE,
                value: sys::GhosttyPointValue {
                    coordinate: sys::GhosttyPointCoordinate { x, y: 0 },
                },
            };
            let mut cell_ref = sys::GhosttyGridRef {
                size: size_of::<sys::GhosttyGridRef>(),
                node: std::ptr::null_mut(),
                x: 0,
                y: 0,
            };
            assert_eq!(
                sys::ghostty_terminal_grid_ref(handle, point, &mut cell_ref),
                sys::GhosttyResult_GHOSTTY_SUCCESS
            );
            let mut cell: sys::GhosttyCell = 0;
            assert_eq!(
                sys::ghostty_grid_ref_cell(&cell_ref, &mut cell),
                sys::GhosttyResult_GHOSTTY_SUCCESS
            );
            let mut tag: sys::GhosttyCellContentTag = sys::GhosttyCellContentTag::MAX;
            assert_eq!(
                sys::ghostty_cell_get(
                    cell,
                    sys::GhosttyCellData_GHOSTTY_CELL_DATA_CONTENT_TAG,
                    (&raw mut tag).cast(),
                ),
                sys::GhosttyResult_GHOSTTY_SUCCESS
            );
            tag
        };

        assert_eq!(
            tag_of(1),
            sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME,
            "a base-plus-combining-mark cell must wear the grapheme tag"
        );
        assert_eq!(
            tag_of(0),
            sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_CODEPOINT,
            "a single-codepoint cell must stay a plain codepoint cell"
        );

        sys::ghostty_terminal_free(handle);
    }
}
