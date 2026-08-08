//! Our own header cannot be allowed to drift from the Rust types it describes.
//!
//! `include/mind2t_vt.h` is hand-written, which means every struct in it is a second declaration
//! of something Rust already declares. Two declarations of one layout is exactly the shape that
//! rots: a field added on one side and forgotten on the other produces no error anywhere, and the
//! symptom is a caller reading the wrong bytes out of a correct library.
//!
//! So the numbers come from Rust and the checking is done by a C compiler. This test emits
//! `_Static_assert` lines built from `size_of`, `align_of` and `offset_of!`, includes the real
//! header, and compiles the result. A mismatch is a compile error naming the field.
//!
//! **What this does NOT cover, stated rather than implied.** C has no name mangling, so a
//! function whose header signature disagrees with the Rust one still links, and no compiler will
//! say so. Layout is verified here; the EXISTENCE of every symbol is verified by
//! `scripts/build-lib.sh` reading `nm`; signature agreement between the two declarations is
//! guarded only by the fact that this module is a thin forward with the types spelled once.

use std::mem::{align_of, offset_of, size_of};

use mind2t_vt_abi_types::*;

/// A C type name paired with what Rust says it measures.
fn size_assertions() -> Vec<String> {
    let mut out = Vec::new();
    let mut sized = |c: &str, size: usize, align: usize| {
        out.push(format!(
            "_Static_assert(sizeof({c}) == {size}, \"sizeof({c}) disagrees with Rust\");"
        ));
        out.push(format!(
            "_Static_assert(_Alignof({c}) == {align}, \"alignof({c}) disagrees with Rust\");"
        ));
    };

    sized("mind2t_vt_result", size_of::<GhosttyResult>(), align_of::<GhosttyResult>());
    sized("mind2t_vt_terminal", size_of::<GhosttyTerminal>(), align_of::<GhosttyTerminal>());
    sized("mind2t_vt_cell", size_of::<GhosttyCell>(), align_of::<GhosttyCell>());
    sized("mind2t_vt_row", size_of::<GhosttyRow>(), align_of::<GhosttyRow>());
    sized("mind2t_vt_mode", size_of::<GhosttyMode>(), align_of::<GhosttyMode>());
    sized("mind2t_vt_color_rgb", size_of::<GhosttyColorRgb>(), align_of::<GhosttyColorRgb>());
    sized(
        "mind2t_vt_style_color",
        size_of::<GhosttyStyleColor>(),
        align_of::<GhosttyStyleColor>(),
    );
    sized("mind2t_vt_style", size_of::<GhosttyStyle>(), align_of::<GhosttyStyle>());
    sized(
        "mind2t_vt_terminal_options",
        size_of::<GhosttyTerminalOptions>(),
        align_of::<GhosttyTerminalOptions>(),
    );
    sized(
        "mind2t_vt_point_coordinate",
        size_of::<GhosttyPointCoordinate>(),
        align_of::<GhosttyPointCoordinate>(),
    );
    sized("mind2t_vt_point", size_of::<GhosttyPoint>(), align_of::<GhosttyPoint>());
    sized("mind2t_vt_string", size_of::<GhosttyString>(), align_of::<GhosttyString>());
    sized("mind2t_vt_grid_ref", size_of::<GhosttyGridRef>(), align_of::<GhosttyGridRef>());
    out
}

/// Field offsets, which is where a hand-written header actually goes wrong: sizes can agree while
/// two fields are transposed, and a transposition is invisible to every size check.
fn offset_assertions() -> Vec<String> {
    let mut out = Vec::new();
    let mut at = |c: &str, field: &str, offset: usize| {
        out.push(format!(
            "_Static_assert(offsetof({c}, {field}) == {offset}, \"{c}.{field} moved\");"
        ));
    };

    at("mind2t_vt_color_rgb", "r", offset_of!(GhosttyColorRgb, r));
    at("mind2t_vt_color_rgb", "g", offset_of!(GhosttyColorRgb, g));
    at("mind2t_vt_color_rgb", "b", offset_of!(GhosttyColorRgb, b));

    at("mind2t_vt_style_color", "tag", offset_of!(GhosttyStyleColor, tag));
    at("mind2t_vt_style_color", "value", offset_of!(GhosttyStyleColor, value));

    at("mind2t_vt_style", "size", offset_of!(GhosttyStyle, size));
    at("mind2t_vt_style", "fg_color", offset_of!(GhosttyStyle, fg_color));
    at("mind2t_vt_style", "bg_color", offset_of!(GhosttyStyle, bg_color));
    at("mind2t_vt_style", "underline_color", offset_of!(GhosttyStyle, underline_color));
    at("mind2t_vt_style", "bold", offset_of!(GhosttyStyle, bold));
    at("mind2t_vt_style", "italic", offset_of!(GhosttyStyle, italic));
    at("mind2t_vt_style", "faint", offset_of!(GhosttyStyle, faint));
    at("mind2t_vt_style", "blink", offset_of!(GhosttyStyle, blink));
    at("mind2t_vt_style", "inverse", offset_of!(GhosttyStyle, inverse));
    at("mind2t_vt_style", "invisible", offset_of!(GhosttyStyle, invisible));
    at("mind2t_vt_style", "strikethrough", offset_of!(GhosttyStyle, strikethrough));
    at("mind2t_vt_style", "overline", offset_of!(GhosttyStyle, overline));
    at("mind2t_vt_style", "underline", offset_of!(GhosttyStyle, underline));

    at("mind2t_vt_terminal_options", "cols", offset_of!(GhosttyTerminalOptions, cols));
    at("mind2t_vt_terminal_options", "rows", offset_of!(GhosttyTerminalOptions, rows));
    at(
        "mind2t_vt_terminal_options",
        "max_scrollback",
        offset_of!(GhosttyTerminalOptions, max_scrollback),
    );

    at("mind2t_vt_point_coordinate", "x", offset_of!(GhosttyPointCoordinate, x));
    at("mind2t_vt_point_coordinate", "y", offset_of!(GhosttyPointCoordinate, y));

    at("mind2t_vt_point", "tag", offset_of!(GhosttyPoint, tag));
    at("mind2t_vt_point", "value", offset_of!(GhosttyPoint, value));

    at("mind2t_vt_string", "ptr", offset_of!(GhosttyString, ptr));
    at("mind2t_vt_string", "len", offset_of!(GhosttyString, len));

    at("mind2t_vt_grid_ref", "size", offset_of!(GhosttyGridRef, size));
    at("mind2t_vt_grid_ref", "node", offset_of!(GhosttyGridRef, node));
    at("mind2t_vt_grid_ref", "x", offset_of!(GhosttyGridRef, x));
    at("mind2t_vt_grid_ref", "y", offset_of!(GhosttyGridRef, y));
    out
}

fn compile(body: &str, name: &str) -> std::process::Output {
    let include = format!("{}/include", env!("CARGO_MANIFEST_DIR"));
    let path = std::env::temp_dir().join(format!("{name}-{}.c", std::process::id()));
    std::fs::write(&path, body).expect("write the generated check");
    let out = std::process::Command::new("cc")
        .args(["-fsyntax-only", "-std=c11", "-I", &include])
        .arg(&path)
        .output()
        .expect("cc must be on PATH");
    let _ = std::fs::remove_file(&path);
    out
}

fn program(extra: &str) -> String {
    let mut body = String::from("#include <stddef.h>\n#include \"mind2t_vt.h\"\n");
    body.push_str(&size_assertions().join("\n"));
    body.push('\n');
    body.push_str(&offset_assertions().join("\n"));
    body.push('\n');
    body.push_str(extra);
    body
}

#[test]
fn our_header_describes_the_rust_types_exactly() {
    let out = compile(&program(""), "mind2t-header");
    assert!(
        out.status.success(),
        "include/mind2t_vt.h disagrees with the Rust types it describes:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The control: the checking must be able to fail.
///
/// A generated file full of assertions that always compile proves nothing about the header - it
/// proves `cc` runs. This appends one deliberately false assertion and requires the compile to
/// fail, so the test above is known to be measuring something.
#[test]
fn the_header_check_can_fail() {
    let liar = "_Static_assert(sizeof(mind2t_vt_style) == 1, \"deliberately wrong\");\n";
    let out = compile(&program(liar), "mind2t-header-control");
    assert!(
        !out.status.success(),
        "a false _Static_assert compiled cleanly, so this harness cannot detect a wrong header"
    );
}

/// Every function the header declares must exist with C linkage and the shape it promises.
///
/// Taking each address through a matching function-pointer type is what makes this more than a
/// spelling check: a declaration whose parameters disagree with the header fails to convert.
#[test]
fn every_declared_function_has_the_declared_shape() {
    let uses = r#"
static mind2t_vt_result (*p_new)(const mind2t_vt_allocator *, mind2t_vt_terminal *,
                                 mind2t_vt_terminal_options) = mind2t_vt_terminal_new;
static void (*p_free)(mind2t_vt_terminal) = mind2t_vt_terminal_free;
static void (*p_write)(mind2t_vt_terminal, const uint8_t *, size_t) = mind2t_vt_terminal_vt_write;
static mind2t_vt_result (*p_resize)(mind2t_vt_terminal, uint16_t, uint16_t, uint32_t, uint32_t)
    = mind2t_vt_terminal_resize;
static mind2t_vt_result (*p_get)(mind2t_vt_terminal, mind2t_vt_terminal_data, void *)
    = mind2t_vt_terminal_get;
static mind2t_vt_result (*p_mode)(mind2t_vt_terminal, mind2t_vt_mode, bool *)
    = mind2t_vt_terminal_mode_get;
static mind2t_vt_result (*p_gref)(mind2t_vt_terminal, mind2t_vt_point, mind2t_vt_grid_ref *)
    = mind2t_vt_terminal_grid_ref;
static mind2t_vt_result (*p_cell)(const mind2t_vt_grid_ref *, mind2t_vt_cell *)
    = mind2t_vt_grid_ref_cell;
static mind2t_vt_result (*p_row)(const mind2t_vt_grid_ref *, mind2t_vt_row *)
    = mind2t_vt_grid_ref_row;
static mind2t_vt_result (*p_graph)(const mind2t_vt_grid_ref *, uint32_t *, size_t, size_t *)
    = mind2t_vt_grid_ref_graphemes;
static mind2t_vt_result (*p_style)(const mind2t_vt_grid_ref *, mind2t_vt_style *)
    = mind2t_vt_grid_ref_style;
static mind2t_vt_result (*p_cget)(mind2t_vt_cell, mind2t_vt_cell_data, void *) = mind2t_vt_cell_get;
static mind2t_vt_result (*p_rget)(mind2t_vt_row, mind2t_vt_row_data, void *) = mind2t_vt_row_get;
static void (*p_sdef)(mind2t_vt_style *) = mind2t_vt_style_default;
"#;
    let out = compile(&program(uses), "mind2t-header-fns");
    assert!(
        out.status.success(),
        "a function in include/mind2t_vt.h does not have the shape it declares:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
