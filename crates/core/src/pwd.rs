//! Purpose: OSC 7, the working directory the child reports.
//! Public surface (crate): `State::osc_pwd`. Read through `Terminal::pwd`.
//! Why this file: the pwd is terminal state with no grid effect at all, which makes it
//!   the one thing in this core invisible to every existing test, so it gets its own
//!   module with its rules written next to the measurements that produced them.
//! NOT responsible for: interpreting the value. It is stored exactly as the child sent it;
//!   decoding the `file://` URI is the embedder's job, and doing it here would diverge
//!   from the oracle on every path with a space or a non-UTF-8 byte in it.
//! Reference: the oracle stores it raw (`../ruuah/src/terminal/stream_terminal.zig`
//!   `reportPwd` into `Terminal.zig` `setPwd`) and clears it on RIS (`fullReset`).
//!   Every rule below is pinned against the real library in
//!   `crates/ghostty/tests/pwd.rs` and, where a corpus case can reach it, in the corpus.

use crate::events::Event;
use crate::terminal::State;

/// The largest payload that survives, measured against the real library on 2026-07-31.
///
/// This is NOT `reportPwd`'s 4096-byte truncation, which is unreachable dead code for
/// OSC 7. The limit that bites is the OSC parser's fixed `[MAX_BUF]u8` capture buffer
/// (`osc.zig`, `MAX_BUF = 2048`), one byte of which goes to the NUL sentinel, so 2047
/// bytes are stored whole and 2048 are stored not at all.
///
/// It has to be enforced HERE rather than inherited from the parser: our vte is built
/// with `std`, where `osc_raw` is an unbounded `Vec` that happily accumulates megabytes.
/// A core that forwards whatever the parser hands it stores a 5000-byte pwd where the
/// oracle stores nothing.
pub(crate) const MAX_PWD_LEN: usize = 2047;

impl State {
    /// OSC 7 ; <uri>. The payload is stored verbatim; an empty one clears.
    ///
    /// Over the limit the whole command is a NO-OP, not a clear and not a truncation. The
    /// oracle never reaches `setPwd` in that case, so a previously reported pwd survives
    /// an over-long report untouched.
    pub(crate) fn osc_pwd(&mut self, params: &[&[u8]]) {
        // Absent rather than empty: `ESC ] 7 ST` with no separator is not a report of an
        // empty pwd, and the oracle's parser produces no command for it at all.
        let Some(payload) = params.get(1) else {
            return;
        };
        if payload.len() > MAX_PWD_LEN {
            return;
        }

        self.pwd.clear();
        self.pwd.extend_from_slice(payload);
        self.push_event(Event::Pwd(self.pwd.clone()));
    }
}

#[cfg(test)]
mod tests {
    use crate::events::Event;
    use crate::terminal::Terminal;

    fn pwd_after(bytes: &[u8]) -> Vec<u8> {
        let mut terminal = Terminal::new(20, 4);
        terminal.write(bytes);
        terminal.pwd().to_vec()
    }

    #[test]
    fn a_report_is_stored_verbatim() {
        assert_eq!(pwd_after(b"\x1b]7;file:///tmp\x1b\\"), b"file:///tmp");
    }

    /// The corpus covers the ordinary cases against the oracle. The limit can only be
    /// unit-tested here, because a 2047-byte literal in `cases.toml` would dwarf the file.
    #[test]
    fn a_payload_of_exactly_the_limit_is_stored_whole() {
        let mut bytes = b"\x1b]7;".to_vec();
        bytes.extend(std::iter::repeat_n(b'a', 2047));
        bytes.extend_from_slice(b"\x1b\\");

        assert_eq!(pwd_after(&bytes).len(), 2047);
    }

    /// One byte more and NOTHING is stored. The oracle's parser drops the command; ours
    /// has to refuse it explicitly, because our vte would hand over the whole payload.
    #[test]
    fn one_byte_past_the_limit_stores_nothing() {
        let mut bytes = b"\x1b]7;".to_vec();
        bytes.extend(std::iter::repeat_n(b'a', 2048));
        bytes.extend_from_slice(b"\x1b\\");

        assert_eq!(pwd_after(&bytes), Vec::<u8>::new());
    }

    /// The distinction the limit exists to get right: an over-long report is a no-op, so
    /// a good pwd is still there afterwards. An implementation that clamps, truncates or
    /// clears on overflow passes both tests above and fails this one.
    #[test]
    fn an_over_long_report_leaves_the_previous_pwd_alone() {
        let mut terminal = Terminal::new(20, 4);
        terminal.write(b"\x1b]7;file:///kept\x1b\\");

        let mut bytes = b"\x1b]7;".to_vec();
        bytes.extend(std::iter::repeat_n(b'a', 3000));
        bytes.extend_from_slice(b"\x1b\\");
        terminal.write(&bytes);
        assert_eq!(terminal.pwd(), b"file:///kept");

        // ...and the parser is still usable for the next one.
        terminal.write(b"\x1b]7;file:///after\x1b\\");
        assert_eq!(terminal.pwd(), b"file:///after");
    }

    #[test]
    fn a_bare_osc7_with_no_payload_is_not_a_clear() {
        let mut terminal = Terminal::new(20, 4);
        terminal.write(b"\x1b]7;file:///tmp\x1b\\");
        terminal.write(b"\x1b]7\x1b\\");
        assert_eq!(terminal.pwd(), b"file:///tmp");
    }

    /// Non-UTF-8 is stored as the bytes it arrived as. A `String` store would have to lose
    /// or replace these, and the snapshot compares bytes exactly, so it would surface as a
    /// divergence rather than as silent corruption.
    #[test]
    fn a_non_utf8_payload_survives_as_bytes() {
        let mut terminal = Terminal::new(20, 4);
        terminal.write(b"\x1b]7;file:///tmp/\xff\xfe\x1b\\");
        assert_eq!(terminal.pwd(), b"file:///tmp/\xff\xfe");
    }

    /// The host learns about it through the event seam, the way it learns about a title.
    /// Polling a string every frame is what this avoids.
    #[test]
    fn a_report_queues_an_event_for_the_embedder() {
        let mut terminal = Terminal::new(20, 4);
        terminal.write(b"\x1b]7;file:///tmp\x1b\\");

        let events = terminal.take_events();
        assert_eq!(events, vec![Event::Pwd(b"file:///tmp".to_vec())]);
    }

    /// A refused report must not reach the seam either: the embedder is told about state
    /// changes, and this one did not happen.
    #[test]
    fn an_over_long_report_queues_no_event() {
        let mut terminal = Terminal::new(20, 4);
        let mut bytes = b"\x1b]7;".to_vec();
        bytes.extend(std::iter::repeat_n(b'a', 3000));
        bytes.extend_from_slice(b"\x1b\\");
        terminal.write(&bytes);

        assert_eq!(terminal.take_events(), Vec::new());
    }
}
