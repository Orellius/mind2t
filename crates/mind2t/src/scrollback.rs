//! Purpose: which key chords move the scrollback viewport, and by how much.
//! Public surface: `Scroll`, `action`, `rows`.
//! Why this file: T4 asks for a scrollback viewport driven from the keyboard, and the host has
//!   two key sources now (AppKit, GTK) with a third coming. The DECISION - is this chord a
//!   scroll, and how far - is identical on every platform and is the part that can be wrong, so
//!   it lives here where a Mac can test it. The call sites are two lines each.
//! NOT responsible for: reading keys, or scrolling. `Session::scroll` owns the viewport and the
//!   pump clamps whatever it is handed, which is why `ToTop` can be a number rather than a query
//!   about how much history exists.
//! Test strategy: the chord table is asserted in BOTH directions - the chords that scroll, and
//!   the bare keys that must NOT, because those have to reach the child.
//!
//! WHY SHIFT AND NOT CTRL, which is a deviation from the plan's wording and is deliberate.
//! The plan asked for "cmd/ctrl+PageUp, Home, End". Cmd is honoured; ctrl is not, and the reason
//! is that ctrl+PageUp and ctrl+Home are REAL terminal sequences that programs consume -
//! `ESC[5;5~` and `ESC[1;5H`. A terminal that swallowed them would break tab switching in TUIs
//! and word-jumps in editors, and the breakage would look like the program's fault. Shift is
//! what every terminal on Linux claims for scrollback (GNOME Terminal, Konsole, Alacritty,
//! Ghostty's own `scroll_page_up` default), so it is the convention rather than a preference.
//!
//! Cmd is additionally claimed on macOS because nothing in a terminal sees cmd at all - it is
//! not a modifier the encoder can even express - so taking it steals nothing from anybody.

use mind2t_vt_pty::key::{KEY_MODS_SHIFT, KEY_MODS_SUPER, Key, KeyMods};

/// What a scrollback chord asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    PageUp,
    PageDown,
    /// The oldest row still in history.
    ToTop,
    /// The live bottom, where new output appears.
    ToBottom,
    /// The nearest OSC 133 prompt mark ABOVE the current viewport - the previous command.
    PrevPrompt,
    /// The nearest prompt mark BELOW the current viewport - the next command.
    NextPrompt,
}

/// Whether a chord is a prompt jump, and which way: `Some(true)` climbs into history.
///
/// The fork every key source needs before it can act, because the two kinds of scroll take
/// different paths - a distance this side computes versus a jump only the pump can resolve
/// against the history the marks live in. Answering `None` for the distance chords is what
/// keeps that decision in one tested place instead of an `if` per host.
///
/// The offset arithmetic is deliberately NOT here: it lives in
/// `mind2t_vt_core::history::prompt_jump_offset`, beside the data, where the pty crate can also
/// reach it. One implementation, two callers, no copy to drift.
pub fn prompt_direction(scroll: Scroll) -> Option<bool> {
    match scroll {
        Scroll::PrevPrompt => Some(true),
        Scroll::NextPrompt => Some(false),
        _ => None,
    }
}

/// The scroll a chord asks for, or `None` when the key belongs to the child.
///
/// `None` is the common answer and the important one: a bare PageUp is how a pager pages, and a
/// terminal that ate it would make `less` unusable while looking like `less` was broken.
pub fn action(key: Key, mods: KeyMods) -> Option<Scroll> {
    // Exactly one of the two claimed modifiers, and no others alongside it. A chord carrying
    // ctrl or alt as well is a different chord and belongs to whatever asked for it - matching
    // on "shift is somewhere in the mods" is how a terminal starts swallowing ctrl+shift+PageUp
    // from a program that wanted it.
    if mods != KEY_MODS_SHIFT && mods != KEY_MODS_SUPER {
        return None;
    }
    match key {
        Key::PageUp | Key::NumpadPageUp => Some(Scroll::PageUp),
        Key::PageDown | Key::NumpadPageDown => Some(Scroll::PageDown),
        Key::Home | Key::NumpadHome => Some(Scroll::ToTop),
        Key::End | Key::NumpadEnd => Some(Scroll::ToBottom),
        // Prompt jumping is CMD-ONLY, and that asymmetry is the point rather than an oversight.
        //
        // Shift+Up and Shift+Down are REAL sequences (`ESC[1;2A` / `ESC[1;2B`) that programs
        // consume - a TUI extends a selection with them. The page/home/end family above can
        // claim shift because every Linux terminal already does and the convention is settled;
        // no such convention exists for shift+arrow, so claiming it here would break selection
        // in TUIs and look like the PROGRAM's fault, which is the most expensive kind of wrong.
        //
        // Cmd takes nothing from anybody: no terminal program can see it, the encoder cannot
        // express it. The cost is that Linux has no prompt-jump chord yet - a real gap, named
        // rather than papered over by taking a sequence that was not ours.
        Key::ArrowUp if mods == KEY_MODS_SUPER => Some(Scroll::PrevPrompt),
        Key::ArrowDown if mods == KEY_MODS_SUPER => Some(Scroll::NextPrompt),
        _ => None,
    }
}

/// How far to scroll, in rows, positive being back into history.
///
/// A page is the viewport MINUS ONE ROW, which is not an off-by-one: paging by the full height
/// leaves no overlap, so the line you were reading when you pressed the key is gone and there is
/// nothing to anchor the eye to. Every pager does it this way. A one-row viewport still scrolls
/// by one, because scrolling by zero is a key that does nothing.
///
/// `ToTop` is a number rather than a query about history depth: the pump clamps the accumulated
/// offset to what exists, so asking for more than there is lands exactly at the top. That keeps
/// this a pure function - no session, no lock, no borrow - which is the whole reason it can be
/// tested from a machine that cannot run either key source.
pub fn rows(scroll: Scroll, viewport_rows: u16) -> i32 {
    let page = i32::from(viewport_rows.saturating_sub(1)).max(1);
    match scroll {
        Scroll::PageUp => page,
        Scroll::PageDown => -page,
        Scroll::ToTop => i32::MAX,
        Scroll::ToBottom => i32::MIN,
        // A prompt jump is not a distance - where it lands depends on where the marks are, and
        // the viewport height says nothing about that. Zero rather than a panic because this is
        // reachable from a host that forgot to route the variant, and a terminal that aborts on
        // a key press is worse than one where a key does nothing. `a_prompt_jump_is_not_a_
        // distance` and the call sites' own routing are what keep it from being reached.
        Scroll::PrevPrompt | Scroll::NextPrompt => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mind2t_vt_pty::key::{KEY_MODS_ALT, KEY_MODS_CTRL};

    #[test]
    fn the_claimed_chords_scroll() {
        assert_eq!(action(Key::PageUp, KEY_MODS_SHIFT), Some(Scroll::PageUp));
        assert_eq!(action(Key::PageDown, KEY_MODS_SHIFT), Some(Scroll::PageDown));
        assert_eq!(action(Key::Home, KEY_MODS_SHIFT), Some(Scroll::ToTop));
        assert_eq!(action(Key::End, KEY_MODS_SHIFT), Some(Scroll::ToBottom));

        // Cmd, which only macOS can produce and which no terminal program can see.
        assert_eq!(action(Key::PageUp, KEY_MODS_SUPER), Some(Scroll::PageUp));
        assert_eq!(action(Key::End, KEY_MODS_SUPER), Some(Scroll::ToBottom));

        // The numpad twins, because a keyboard with a numpad and numlock off sends those and a
        // table that forgot them scrolls from one half of the keyboard only.
        assert_eq!(action(Key::NumpadPageUp, KEY_MODS_SHIFT), Some(Scroll::PageUp));
        assert_eq!(action(Key::NumpadEnd, KEY_MODS_SHIFT), Some(Scroll::ToBottom));
    }

    /// The control, and it is the half that matters more.
    ///
    /// Every one of these has to reach the child. A bare PageUp is how a pager pages; ctrl+Home
    /// is a word-jump in an editor; ctrl+PageUp switches tabs in several TUIs. A terminal that
    /// swallowed any of them would look like the PROGRAM was broken, which is the most expensive
    /// kind of wrong because nobody suspects the terminal.
    #[test]
    fn everything_else_belongs_to_the_child() {
        for key in [Key::PageUp, Key::PageDown, Key::Home, Key::End] {
            assert_eq!(action(key, 0), None, "{key:?} with no modifier is the child's");
            assert_eq!(action(key, KEY_MODS_CTRL), None, "ctrl+{key:?} is a real sequence");
            assert_eq!(action(key, KEY_MODS_ALT), None, "alt+{key:?} is a real sequence");
            assert_eq!(
                action(key, KEY_MODS_CTRL | KEY_MODS_SHIFT),
                None,
                "ctrl+shift+{key:?} is a different chord, not a scroll"
            );
        }
        assert_eq!(action(Key::A, KEY_MODS_SHIFT), None);
        assert_eq!(action(Key::ArrowUp, KEY_MODS_SHIFT), None);
    }

    #[test]
    fn a_page_keeps_one_row_of_overlap() {
        assert_eq!(rows(Scroll::PageUp, 40), 39);
        assert_eq!(rows(Scroll::PageDown, 40), -39);
        assert_eq!(rows(Scroll::PageUp, 24), 23);
    }

    /// A degenerate viewport still moves. Scrolling by zero is a key that does nothing, which is
    /// indistinguishable from a chord that was never wired.
    #[test]
    fn a_tiny_viewport_still_scrolls_by_a_row() {
        assert_eq!(rows(Scroll::PageUp, 1), 1);
        assert_eq!(rows(Scroll::PageUp, 0), 1);
        assert_eq!(rows(Scroll::PageDown, 0), -1);
    }

    #[test]
    fn the_ends_ask_for_more_than_exists_and_let_the_pump_clamp() {
        assert_eq!(rows(Scroll::ToTop, 40), i32::MAX);
        assert_eq!(rows(Scroll::ToBottom, 40), i32::MIN);
    }

    #[test]
    fn cmd_arrows_jump_between_prompts() {
        assert_eq!(action(Key::ArrowUp, KEY_MODS_SUPER), Some(Scroll::PrevPrompt));
        assert_eq!(action(Key::ArrowDown, KEY_MODS_SUPER), Some(Scroll::NextPrompt));
    }

    /// The control for the chord choice, and the reason the arrows are cmd-only.
    ///
    /// Shift+Up is `ESC[1;2A` - a TUI extends a selection with it. If this ever starts
    /// answering `Some`, selection breaks in every full-screen program and looks like the
    /// program's bug. That is a louder failure than losing the jump, so it is pinned here.
    #[test]
    fn shift_arrows_still_belong_to_the_child() {
        assert_eq!(action(Key::ArrowUp, KEY_MODS_SHIFT), None);
        assert_eq!(action(Key::ArrowDown, KEY_MODS_SHIFT), None);
        assert_eq!(action(Key::ArrowUp, 0), None);
        assert_eq!(action(Key::ArrowDown, KEY_MODS_CTRL), None);
    }

    /// The direction fork, which is the half that lives in this crate. The offset arithmetic
    /// it feeds is tested in `mind2t_vt_core::history` against literal slices.
    #[test]
    fn only_the_arrow_chords_are_prompt_jumps() {
        assert_eq!(prompt_direction(Scroll::PrevPrompt), Some(true));
        assert_eq!(prompt_direction(Scroll::NextPrompt), Some(false));
        // The distance chords must answer None, or a host would route a page scroll to the
        // pump as a jump and the page keys would quietly stop paging.
        for distance in [Scroll::PageUp, Scroll::PageDown, Scroll::ToTop, Scroll::ToBottom] {
            assert_eq!(prompt_direction(distance), None, "{distance:?} is a distance");
        }
    }

    /// `rows` has no answer for a prompt jump, because the distance is not a function of the
    /// viewport - it comes from where the marks actually are. Pinned so a future edit that
    /// wires a jump through `rows` fails here rather than scrolling by a page.
    #[test]
    fn a_prompt_jump_is_not_a_distance() {
        // `rows` answers zero for a jump - it cannot know the distance. The guard that keeps
        // that zero unreachable is `apply_scroll` routing on `prompt_direction` first.
        assert_eq!(rows(Scroll::PrevPrompt, 40), 0);
        assert_eq!(rows(Scroll::NextPrompt, 40), 0);
        assert!(prompt_direction(Scroll::PrevPrompt).is_some());
    }
}
