//! Purpose: track which parts of the grid are prompt, input and output, per OSC 133.
//! Public surface: `State::osc`, `State::index`, `State::wrap`, `State::set_row_semantic`.
//! Why this file: the semantic prompt state is small but its rules live in three places at
//!   once -- an OSC action, the cell being printed, and the row being entered -- and putting
//!   them together is what makes them checkable against each other. `terminal.rs` was also
//!   already near the 500-line ceiling.
//! NOT responsible for: writing cells (`terminal.rs`), row storage (`grid.rs`), or the
//!   reflow rule (`reflow.rs`, which carries its own measurement).
//! Test strategy: every rule here is a corpus case measured against libghostty-vt, and the
//!   oracle's own behaviour is pinned in `crates/ghostty/tests/semantic.rs`.

use ruuah_vt_snapshot::{RowSemantic, Semantic};

use crate::cell::Cell;
use crate::terminal::State;

impl State {
    /// Routes an OSC. Only 133 is understood; everything else is absorbed silently, which is
    /// what a terminal must do with an OSC it does not implement.
    pub(crate) fn osc(&mut self, params: &[&[u8]], blank: Cell) {
        if params.first().copied() != Some(b"133".as_slice()) {
            return;
        }
        let Some(field) = params.get(1) else {
            return;
        };
        // The action field is the letter ALONE. Upstream demands the byte after the letter
        // be a semicolon or the end (semantic_prompt.zig:352) and drops the whole command
        // otherwise; vte has already split on semicolons, so any longer field here is that
        // trailing garbage (finding 29).
        let &[action] = *field else {
            return;
        };
        // L is the one action that rejects options outright (semantic_prompt.zig:367).
        if action == b'L' && params.len() > 2 {
            return;
        }

        match action {
            // A is fresh-line-then-new-prompt. N may terminate a previous command first, but
            // this core does no command tracking, so upstream treats it as A and so does this.
            b'A' | b'N' => {
                self.semantic_fresh_line(blank);
                self.start_prompt(prompt_mark(params));
            }
            // P is an explicit prompt start with no fresh line.
            b'P' => self.start_prompt(prompt_mark(params)),
            b'B' => self.start_input(false),
            b'I' => self.start_input(true),
            b'C' => {
                self.end_input();
                // Execution begins here, which is the ONE moment the typed command is
                // final -- the event is how S4's history capture learns it without
                // guessing from block geometry (adjacent prompt runs merge, and at
                // the grid bottom a new prompt scrolls in on the SAME row).
                self.push_event(crate::events::Event::CommandStart);
            }
            b'D' => self.start_output(),
            b'L' => self.semantic_fresh_line(blank),
            _ => {}
        }
    }

    /// IND: down one row, applying the rule for entering a row in a non-output state.
    ///
    /// Every plain line feed goes through here rather than through `Screen::line_feed`
    /// directly, because the rule belongs to the movement and not to any one caller.
    pub(crate) fn index(&mut self, blank: Cell) {
        self.screen_mut().line_feed(blank);
        self.enter_row();
    }

    /// A soft wrap, which is an `index` that preserves the cursor's semantic state.
    ///
    /// The save-and-restore is not decoration. `index` may reset the state to output when the
    /// input was declared to end at end-of-line, and a soft wrap is not an end of line, so
    /// upstream's `printWrap` restores what it saved. That is the whole of why `B` and `I`
    /// behave identically everywhere except across a wrap.
    pub(crate) fn wrap(&mut self, blank: Cell) {
        let content = self.screen().semantic_content;
        let clear_at_eol = self.screen().semantic_clear_at_eol;

        self.screen_mut().wrap_line(blank);
        self.enter_row();

        self.screen_mut().semantic_content = content;
        self.screen_mut().semantic_clear_at_eol = clear_at_eol;
        if content == Semantic::Prompt {
            self.set_row_semantic(RowSemantic::PromptContinuation);
        }
    }

    /// Marks the row the cursor is on.
    pub(crate) fn set_row_semantic(&mut self, mark: RowSemantic) {
        let y = self.screen().y;
        if let Some(meta) = self.screen_mut().grid.row_meta_mut(y) {
            meta.semantic_prompt = mark;
        }
    }

    fn row_semantic(&self) -> RowSemantic {
        let y = self.screen().y;
        self.screen().grid.row_meta(y).semantic_prompt
    }

    /// Applied on arriving at a new row while the cursor is not in the output state.
    ///
    /// The continuation mark is a workaround for shells that emit no continuation prompt of
    /// their own: without it a wrapped command line would look like unrelated rows. It can be
    /// a false positive, which upstream then undoes in `end_input` when output starts at
    /// column zero.
    fn enter_row(&mut self) {
        if self.screen().semantic_content == Semantic::Output {
            return;
        }
        if self.screen().semantic_clear_at_eol {
            self.screen_mut().semantic_content = Semantic::Output;
            self.screen_mut().semantic_clear_at_eol = false;
        } else {
            self.set_row_semantic(RowSemantic::PromptContinuation);
        }
    }

    fn start_prompt(&mut self, mark: RowSemantic) {
        self.screen_mut().semantic_content = Semantic::Prompt;
        self.screen_mut().semantic_clear_at_eol = false;
        self.set_row_semantic(mark);
    }

    fn start_input(&mut self, clear_at_eol: bool) {
        self.screen_mut().semantic_content = Semantic::Input;
        self.screen_mut().semantic_clear_at_eol = clear_at_eol;
    }

    fn start_output(&mut self) {
        self.screen_mut().semantic_content = Semantic::Output;
        self.screen_mut().semantic_clear_at_eol = false;
    }

    /// C: end of input, start of output.
    ///
    /// The second half undoes a continuation mark this row may have been given on the way in:
    /// output starting at column zero on a marked row means the shell is writing over the
    /// prompt rather than after it. The test is positional, so the same C mid-row leaves the
    /// mark standing.
    fn end_input(&mut self) {
        self.start_output();
        if self.screen().x == 0 && self.row_semantic() != RowSemantic::None {
            self.set_row_semantic(RowSemantic::None);
        }
    }

    /// Moves to a new line unless the cursor is already at the left margin.
    fn semantic_fresh_line(&mut self, blank: Cell) {
        if self.screen().x == 0 {
            return;
        }
        self.screen_mut().x = 0;
        self.index(blank);
        self.last_print = None;
    }
}

/// The row mark an `A`, `N` or `P` asks for, from its `k=` option.
///
/// A right-side prompt marks a primary line and a secondary one marks a continuation, which
/// is why this maps four kinds onto two marks rather than passing the kind along.
fn prompt_mark(params: &[&[u8]]) -> RowSemantic {
    for option in params.iter().skip(2) {
        if let Some(value) = option.strip_prefix(b"k=") {
            // The value is taken only when it is exactly one byte (semantic_prompt.zig:203);
            // anything longer falls back to the initial kind, a plain prompt (finding 29).
            return match value {
                b"c" | b"s" => RowSemantic::PromptContinuation,
                _ => RowSemantic::Prompt,
            };
        }
    }
    RowSemantic::Prompt
}
