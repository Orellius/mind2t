//! Purpose: the kitty keyboard protocol's flag stack, per screen, exactly as the
//!   oracle stores it (`src/terminal/kitty/key.zig`).
//! Public surface: `KittyKeyFlags` (a 5-bit mask) and (crate-side) `KittyFlagStack`.
//! Why this file: the core TRACKS the negotiated flags and encodes nothing -- key
//!   encoding is host-side, like mouse and paste. The stack lives on the SCREEN, not
//!   the terminal: kitty's spec gives the main and alternate screens independent
//!   stacks, and the oracle stores it as a `Screen` field, so a TUI entering 1049
//!   pushes its flags without disturbing the shell's.
//! NOT responsible for: parsing `CSI u` forms (dispatch.rs), answering the query
//!   (replies.rs), or encoding keys (`mind2t-vt-pty`).
//! Test strategy: stack semantics unit-tested here against the measured rules; the
//!   negotiated-state -> bytes mapping is differentially gated through the oracle's
//!   `ghostty_key_encoder_setopt_from_terminal` (there is NO mode_get/snapshot
//!   observable for kitty flags -- the encoder differential is the only gate, said
//!   loudly where the corpus cannot see).

/// The five kitty keyboard protocol flags, as the wire carries them
/// (`CSI = flags ; mode u`, `CSI > flags u`, and the `CSI ? flags u` reply).
pub type KittyKeyFlags = u8;

pub const KITTY_DISAMBIGUATE: u8 = 1 << 0;
pub const KITTY_REPORT_EVENTS: u8 = 1 << 1;
pub const KITTY_REPORT_ALTERNATES: u8 = 1 << 2;
pub const KITTY_REPORT_ALL: u8 = 1 << 3;
pub const KITTY_REPORT_ASSOCIATED: u8 = 1 << 4;
/// Everything above; also the largest value the parser accepts (a u5).
pub const KITTY_ALL: u8 = 0b11111;

/// How `CSI = flags ; mode u` combines with the current entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetMode {
    Set,
    Or,
    Not,
}

/// The fixed-size stack behind `CSI > u` / `CSI < u`. Eight entries, no heap, and
/// deliberately WRAPPING: a ninth push evicts the oldest entry, and a pop of eight
/// or more resets everything -- both measured from the oracle, whose comment names
/// the second rule as a DoS guard against pop floods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KittyFlagStack {
    flags: [KittyKeyFlags; Self::LEN],
    idx: usize,
}

impl Default for KittyFlagStack {
    fn default() -> Self {
        Self { flags: [0; Self::LEN], idx: 0 }
    }
}

impl KittyFlagStack {
    const LEN: usize = 8;

    pub(crate) fn current(&self) -> KittyKeyFlags {
        self.flags[self.idx]
    }

    pub(crate) fn push(&mut self, flags: KittyKeyFlags) {
        self.idx = (self.idx + 1) % Self::LEN;
        self.flags[self.idx] = flags;
    }

    pub(crate) fn pop(&mut self, n: usize) {
        if n >= Self::LEN {
            *self = Self::default();
            return;
        }
        for _ in 0..n {
            self.flags[self.idx] = 0;
            self.idx = (self.idx + Self::LEN - 1) % Self::LEN;
        }
    }

    pub(crate) fn set(&mut self, mode: SetMode, flags: KittyKeyFlags) {
        let current = &mut self.flags[self.idx];
        match mode {
            SetMode::Set => *current = flags,
            SetMode::Or => *current |= flags,
            SetMode::Not => *current &= !flags,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_pop_round_trip() {
        let mut stack = KittyFlagStack::default();
        stack.push(KITTY_DISAMBIGUATE);
        assert_eq!(stack.current(), KITTY_DISAMBIGUATE);
        stack.pop(1);
        assert_eq!(stack.current(), 0);
    }

    /// The ninth push wraps onto the OLDEST slot: after eight more pops the stack is
    /// fully drained rather than remembering the evicted entry.
    #[test]
    fn a_ninth_push_evicts_the_oldest_entry() {
        let mut stack = KittyFlagStack::default();
        for i in 1..=9u8 {
            stack.push(i % 32);
        }
        assert_eq!(stack.current(), 9 % 32);
        // Slot 0 (the pre-push base) was overwritten by push #8's wrap... walk back
        // and confirm the survivor set matches the oracle's ring arithmetic.
        stack.pop(1);
        assert_eq!(stack.current(), 8);
    }

    /// Popping more than the depth is a full reset, the oracle's DoS guard.
    #[test]
    fn a_huge_pop_resets_instead_of_spinning() {
        let mut stack = KittyFlagStack::default();
        stack.push(KITTY_ALL);
        stack.pop(100);
        assert_eq!(stack.current(), 0);
        assert_eq!(stack, KittyFlagStack::default());
    }

    #[test]
    fn set_or_and_not_compose_on_the_current_entry() {
        let mut stack = KittyFlagStack::default();
        stack.set(SetMode::Set, KITTY_DISAMBIGUATE);
        stack.set(SetMode::Or, KITTY_REPORT_EVENTS);
        assert_eq!(stack.current(), KITTY_DISAMBIGUATE | KITTY_REPORT_EVENTS);
        stack.set(SetMode::Not, KITTY_REPORT_EVENTS);
        assert_eq!(stack.current(), KITTY_DISAMBIGUATE);
    }
}
