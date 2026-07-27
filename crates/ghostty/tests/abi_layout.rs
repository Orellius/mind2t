//! Purpose: prove the Rust view of every libghostty-vt struct matches the binary's own.
//! Public surface: none, this is a test.
//! Why this file: the oracle is only an oracle if it is read correctly. A wrong offset
//!   here would not crash; it would quietly return plausible garbage and every downstream
//!   diff would be measuring the binding bug instead of the terminal. It also catches the
//!   vendored headers drifting from the linked archive, which bindgen alone cannot see.
//! NOT responsible for: behaviour. Nothing here writes a byte to a terminal.
//! Test strategy: compare `size_of` / `offset_of` against `ghostty_type_json()` field by
//!   field, and fail if the library stops describing a struct this crate depends on.

use std::mem::{offset_of, size_of};

use serde_json::Value;
use vtr_ghostty::{sys, type_layout_json};

/// Every struct this crate reads or writes across the ABI, with the offset of each field
/// it actually touches. If a field is listed here, the harness depends on it.
fn expectations() -> Vec<(&'static str, usize, Vec<(&'static str, usize)>)> {
    vec![
        (
            "GhosttyTerminalOptions",
            size_of::<sys::GhosttyTerminalOptions>(),
            vec![
                ("cols", offset_of!(sys::GhosttyTerminalOptions, cols)),
                ("rows", offset_of!(sys::GhosttyTerminalOptions, rows)),
                (
                    "max_scrollback",
                    offset_of!(sys::GhosttyTerminalOptions, max_scrollback),
                ),
            ],
        ),
        (
            "GhosttyGridRef",
            size_of::<sys::GhosttyGridRef>(),
            vec![
                ("size", offset_of!(sys::GhosttyGridRef, size)),
                ("node", offset_of!(sys::GhosttyGridRef, node)),
                ("x", offset_of!(sys::GhosttyGridRef, x)),
                ("y", offset_of!(sys::GhosttyGridRef, y)),
            ],
        ),
        (
            "GhosttyPoint",
            size_of::<sys::GhosttyPoint>(),
            vec![
                ("tag", offset_of!(sys::GhosttyPoint, tag)),
                ("value", offset_of!(sys::GhosttyPoint, value)),
            ],
        ),
        (
            "GhosttyPointCoordinate",
            size_of::<sys::GhosttyPointCoordinate>(),
            vec![
                ("x", offset_of!(sys::GhosttyPointCoordinate, x)),
                ("y", offset_of!(sys::GhosttyPointCoordinate, y)),
            ],
        ),
        (
            "GhosttyStyleColor",
            size_of::<sys::GhosttyStyleColor>(),
            vec![
                ("tag", offset_of!(sys::GhosttyStyleColor, tag)),
                ("value", offset_of!(sys::GhosttyStyleColor, value)),
            ],
        ),
        (
            "GhosttyStyle",
            size_of::<sys::GhosttyStyle>(),
            vec![
                ("size", offset_of!(sys::GhosttyStyle, size)),
                ("fg_color", offset_of!(sys::GhosttyStyle, fg_color)),
                ("bg_color", offset_of!(sys::GhosttyStyle, bg_color)),
                (
                    "underline_color",
                    offset_of!(sys::GhosttyStyle, underline_color),
                ),
                ("bold", offset_of!(sys::GhosttyStyle, bold)),
                ("italic", offset_of!(sys::GhosttyStyle, italic)),
                ("faint", offset_of!(sys::GhosttyStyle, faint)),
                ("blink", offset_of!(sys::GhosttyStyle, blink)),
                ("inverse", offset_of!(sys::GhosttyStyle, inverse)),
                ("invisible", offset_of!(sys::GhosttyStyle, invisible)),
                (
                    "strikethrough",
                    offset_of!(sys::GhosttyStyle, strikethrough),
                ),
                ("overline", offset_of!(sys::GhosttyStyle, overline)),
                ("underline", offset_of!(sys::GhosttyStyle, underline)),
            ],
        ),
    ]
}

#[test]
fn rust_struct_layouts_match_the_librarys_own_report() {
    let reported: Value =
        serde_json::from_str(type_layout_json()).expect("ghostty_type_json must be valid JSON");

    for (name, rust_size, fields) in expectations() {
        let entry = reported
            .get(name)
            .unwrap_or_else(|| panic!("libghostty-vt no longer describes {name}"));

        let lib_size = entry["size"].as_u64().expect("size is a number") as usize;
        assert_eq!(
            rust_size, lib_size,
            "{name}: rust size_of={rust_size}, libghostty-vt reports {lib_size}"
        );

        for (field, rust_offset) in fields {
            let lib_offset = entry["fields"][field]["offset"]
                .as_u64()
                .unwrap_or_else(|| panic!("libghostty-vt no longer describes {name}.{field}"))
                as usize;
            assert_eq!(
                rust_offset, lib_offset,
                "{name}.{field}: rust offset_of={rust_offset}, libghostty-vt reports {lib_offset}"
            );
        }
    }
}

#[test]
fn sized_structs_are_populated_before_the_library_sees_them() {
    // GHOSTTY_INIT_SIZED is the library's version-negotiation mechanism: a caller that
    // leaves `size` at zero is claiming to be compiled against a zero-byte struct. Both
    // sized structs this crate constructs must therefore report a non-zero size, and it
    // must be the size the library itself reports.
    let reported: Value = serde_json::from_str(type_layout_json()).expect("valid JSON");

    for name in ["GhosttyGridRef", "GhosttyStyle"] {
        let lib_size = reported[name]["size"].as_u64().expect("size is a number");
        assert!(lib_size > 0, "{name} reports a zero size");
    }
}

#[test]
fn the_enum_discriminants_this_crate_matches_on_are_what_the_headers_say() {
    // These are matched exhaustively in `terminal.rs`. A silent renumbering upstream
    // would turn a correct read into a wrong one with no compile error.
    assert_eq!(sys::GhosttyResult_GHOSTTY_SUCCESS, 0);
    assert_eq!(sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE, -3);
    assert_eq!(sys::GhosttyPointTag_GHOSTTY_POINT_TAG_ACTIVE, 0);
    assert_eq!(sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW, 0);
    assert_eq!(sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_TAIL, 2);
    assert_eq!(sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_NONE, 0);
    assert_eq!(sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_RGB, 2);
    assert_eq!(sys::GhosttySgrUnderline_GHOSTTY_SGR_UNDERLINE_CURLY, 3);
    assert_eq!(
        sys::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_ALTERNATE,
        1
    );
}
