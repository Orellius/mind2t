//! Purpose: prove the types mind2t-vt PUBLISHES match the ones libghostty-vt publishes.
//! Public surface: none, this is a test.
//! Why this file: slice 6's claim is that a program compiled against `ghostty/vt/*.h` can
//!   link `libmind2t-vt.a` instead. That is a claim about byte layout, and it was previously
//!   supported only by the fact that a human transcribed the numbers carefully. Here they are
//!   checked against the real library's own `ghostty_type_json()` -- the same self-description
//!   `abi_layout.rs` uses to verify the bindings, applied to the other direction.
//! NOT responsible for: behaviour across the ABI, which is
//!   `crates/abi/tests/differential.rs`.
//! Test strategy: for every struct mind2t-vt publishes, compare `size_of` and each
//!   `offset_of` against the JSON. A struct the library stops describing is a failure, not a
//!   silent skip -- otherwise the test evaporates the day the report changes shape.
//!
//! This test lives here rather than in `crates/abi` because only this crate links
//! libghostty-vt, and `mind2t-vt-abi-types` deliberately depends on nothing so it can be
//! pulled in beside the oracle. Linking `mind2t-vt-abi` itself here would collide: both
//! define `ghostty_terminal_new`.

use std::mem::{align_of, offset_of, size_of};

use mind2t_vt_abi_types as ours;
use mind2t_vt_ghostty::type_layout_json;
use serde_json::Value;

/// Every published struct, with the offset of each field a consumer can name.
fn expectations() -> Vec<(&'static str, usize, usize, Vec<(&'static str, usize)>)> {
    vec![
        (
            "GhosttyTerminalOptions",
            size_of::<ours::GhosttyTerminalOptions>(),
            align_of::<ours::GhosttyTerminalOptions>(),
            vec![
                ("cols", offset_of!(ours::GhosttyTerminalOptions, cols)),
                ("rows", offset_of!(ours::GhosttyTerminalOptions, rows)),
                (
                    "max_scrollback",
                    offset_of!(ours::GhosttyTerminalOptions, max_scrollback),
                ),
            ],
        ),
        (
            "GhosttyGridRef",
            size_of::<ours::GhosttyGridRef>(),
            align_of::<ours::GhosttyGridRef>(),
            vec![
                ("size", offset_of!(ours::GhosttyGridRef, size)),
                ("node", offset_of!(ours::GhosttyGridRef, node)),
                ("x", offset_of!(ours::GhosttyGridRef, x)),
                ("y", offset_of!(ours::GhosttyGridRef, y)),
            ],
        ),
        (
            "GhosttyStyle",
            size_of::<ours::GhosttyStyle>(),
            align_of::<ours::GhosttyStyle>(),
            vec![
                ("size", offset_of!(ours::GhosttyStyle, size)),
                ("fg_color", offset_of!(ours::GhosttyStyle, fg_color)),
                ("bg_color", offset_of!(ours::GhosttyStyle, bg_color)),
                (
                    "underline_color",
                    offset_of!(ours::GhosttyStyle, underline_color),
                ),
                ("bold", offset_of!(ours::GhosttyStyle, bold)),
                ("underline", offset_of!(ours::GhosttyStyle, underline)),
            ],
        ),
        (
            "GhosttyStyleColor",
            size_of::<ours::GhosttyStyleColor>(),
            align_of::<ours::GhosttyStyleColor>(),
            vec![
                ("tag", offset_of!(ours::GhosttyStyleColor, tag)),
                ("value", offset_of!(ours::GhosttyStyleColor, value)),
            ],
        ),
        (
            "GhosttyColorRgb",
            size_of::<ours::GhosttyColorRgb>(),
            align_of::<ours::GhosttyColorRgb>(),
            vec![
                ("r", offset_of!(ours::GhosttyColorRgb, r)),
                ("g", offset_of!(ours::GhosttyColorRgb, g)),
                ("b", offset_of!(ours::GhosttyColorRgb, b)),
            ],
        ),
        (
            "GhosttyPoint",
            size_of::<ours::GhosttyPoint>(),
            align_of::<ours::GhosttyPoint>(),
            vec![
                ("tag", offset_of!(ours::GhosttyPoint, tag)),
                ("value", offset_of!(ours::GhosttyPoint, value)),
            ],
        ),
    ]
}

fn described() -> Value {
    serde_json::from_str(type_layout_json()).expect("the library reports valid JSON")
}

fn find<'a>(report: &'a Value, name: &str) -> &'a Value {
    report.get(name).unwrap_or_else(|| {
        panic!("libghostty-vt no longer describes {name}; this test cannot verify parity")
    })
}

#[test]
fn every_published_struct_matches_the_library() {
    let report = described();

    for (name, size, align, fields) in expectations() {
        let described = find(&report, name);

        let their_size = described
            .get("size")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| panic!("{name} has no size in the report"));
        assert_eq!(
            their_size as usize, size,
            "{name}: libghostty-vt says {their_size} bytes, mind2t-vt publishes {size}"
        );

        // The key is "align" (types.zig:143), and the lookup is required rather than
        // if-let: this assertion spent its first weeks dead because it read "alignment",
        // which the report never contains, and an assertion that silently skips is
        // indistinguishable from one that passes (finding 24). Proven by mutation: with
        // the old key, an align expectation of +999 passed.
        let their_align = described
            .get("align")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| panic!("{name} has no align in the report"));
        assert_eq!(
            their_align as usize, align,
            "{name}: alignment differs, so a consumer's struct would be laid out differently"
        );

        for (field, offset) in fields {
            let their_offset = described["fields"][field]["offset"]
                .as_u64()
                .unwrap_or_else(|| panic!("{name}.{field} is not described"));
            assert_eq!(
                their_offset as usize, offset,
                "{name}.{field}: libghostty-vt puts it at {their_offset}, mind2t-vt at {offset}"
            );
        }
    }
}

/// The report has to actually contain what this test reads, or the test above passes by
/// finding nothing. Cheap, and it is the difference between a parity check and a no-op.
#[test]
fn the_library_still_describes_the_structs_this_test_reads() {
    let report = described();
    let structs = report
        .as_object()
        .expect("the report is an object keyed by struct name");

    for (name, ..) in expectations() {
        assert!(
            structs.contains_key(name),
            "{name} vanished from the report"
        );
    }
    assert!(
        structs.len() >= expectations().len(),
        "the report shrank below what parity needs"
    );
}
