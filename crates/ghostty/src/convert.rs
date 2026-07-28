//! Purpose: turn libghostty-vt's C enums and structs into the snapshot's own types.
//! Public surface: `convert_style`, `convert_color`, `convert_wide`, `convert_semantic`,
//!   `convert_row_semantic`, `convert_underline`.
//! Why this file: every one of these is a total match over an ABI enum, and an unknown
//!   variant has to become an error rather than a default -- a library that grew a fourth
//!   cell width would otherwise be silently read as `Narrow`. Keeping them together makes
//!   that discipline checkable at a glance, and keeps `terminal.rs` to the readout itself.
//! NOT responsible for: reading anything (`terminal.rs`) or judging it (`ruuah-vt-snapshot`).
//! Test strategy: exercised through `tests/oracle.rs` and `tests/semantic.rs`, which drive
//!   real byte streams and assert on the converted values.

use ruuah_vt_snapshot::{Color, RowSemantic, Semantic, Style, Underline, Wide};

use crate::sys;
use crate::terminal::Error;

pub(crate) fn convert_style(raw: &sys::GhosttyStyle) -> Result<Style, Error> {
    Ok(Style {
        fg: convert_color(&raw.fg_color)?,
        bg: convert_color(&raw.bg_color)?,
        underline_color: convert_color(&raw.underline_color)?,
        bold: raw.bold,
        italic: raw.italic,
        faint: raw.faint,
        blink: raw.blink,
        inverse: raw.inverse,
        invisible: raw.invisible,
        strikethrough: raw.strikethrough,
        overline: raw.overline,
        underline: convert_underline(raw.underline)?,
    })
}

pub(crate) fn convert_color(raw: &sys::GhosttyStyleColor) -> Result<Color, Error> {
    match raw.tag {
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_NONE => Ok(Color::Default),
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_PALETTE => {
            Ok(Color::Palette(unsafe { raw.value.palette }))
        }
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_RGB => {
            let rgb = unsafe { raw.value.rgb };
            Ok(Color::Rgb {
                r: rgb.r,
                g: rgb.g,
                b: rgb.b,
            })
        }
        other => Err(Error::UnknownEnum {
            kind: "GhosttyStyleColorTag",
            value: other,
        }),
    }
}

pub(crate) fn convert_wide(raw: sys::GhosttyCellWide) -> Result<Wide, Error> {
    match raw {
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW => Ok(Wide::Narrow),
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_WIDE => Ok(Wide::Wide),
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_TAIL => Ok(Wide::SpacerTail),
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_HEAD => Ok(Wide::SpacerHead),
        other => Err(Error::UnknownEnum {
            kind: "GhosttyCellWide",
            value: other,
        }),
    }
}

pub(crate) fn convert_semantic(raw: sys::GhosttyCellSemanticContent) -> Result<Semantic, Error> {
    match raw {
        sys::GhosttyCellSemanticContent_GHOSTTY_CELL_SEMANTIC_OUTPUT => Ok(Semantic::Output),
        sys::GhosttyCellSemanticContent_GHOSTTY_CELL_SEMANTIC_INPUT => Ok(Semantic::Input),
        sys::GhosttyCellSemanticContent_GHOSTTY_CELL_SEMANTIC_PROMPT => Ok(Semantic::Prompt),
        other => Err(Error::UnknownEnum {
            kind: "GhosttyCellSemanticContent",
            value: other,
        }),
    }
}

pub(crate) fn convert_row_semantic(
    raw: sys::GhosttyRowSemanticPrompt,
) -> Result<RowSemantic, Error> {
    match raw {
        sys::GhosttyRowSemanticPrompt_GHOSTTY_ROW_SEMANTIC_NONE => Ok(RowSemantic::None),
        sys::GhosttyRowSemanticPrompt_GHOSTTY_ROW_SEMANTIC_PROMPT => Ok(RowSemantic::Prompt),
        sys::GhosttyRowSemanticPrompt_GHOSTTY_ROW_SEMANTIC_PROMPT_CONTINUATION => {
            Ok(RowSemantic::PromptContinuation)
        }
        other => Err(Error::UnknownEnum {
            kind: "GhosttyRowSemanticPrompt",
            value: other,
        }),
    }
}

pub(crate) fn convert_underline(raw: i32) -> Result<Underline, Error> {
    match raw as u32 {
        sys::GhosttySgrUnderline_GHOSTTY_SGR_UNDERLINE_NONE => Ok(Underline::None),
        sys::GhosttySgrUnderline_GHOSTTY_SGR_UNDERLINE_SINGLE => Ok(Underline::Single),
        sys::GhosttySgrUnderline_GHOSTTY_SGR_UNDERLINE_DOUBLE => Ok(Underline::Double),
        sys::GhosttySgrUnderline_GHOSTTY_SGR_UNDERLINE_CURLY => Ok(Underline::Curly),
        sys::GhosttySgrUnderline_GHOSTTY_SGR_UNDERLINE_DOTTED => Ok(Underline::Dotted),
        sys::GhosttySgrUnderline_GHOSTTY_SGR_UNDERLINE_DASHED => Ok(Underline::Dashed),
        other => Err(Error::UnknownEnum {
            kind: "GhosttySgrUnderline",
            value: other,
        }),
    }
}
