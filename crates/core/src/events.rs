//! Purpose: host-facing events -- things a terminal ASKS ITS EMBEDDER TO DO, as opposed
//! to grid state it owns. OSC 52 (set clipboard), OSC 9 / OSC 777;notify (post a
//! notification), BEL.
//! Public surface: `Event`, `Terminal::take_events` (in `terminal.rs`).
//! Why this file: the core does no I/O, so these cannot be side effects -- they queue
//!   here and the pump drains them across the thread boundary. The queue is bounded and
//!   drops the OLDEST on overflow: for a clipboard the newest write is the true state,
//!   and a notification storm losing its head is strictly better than unbounded memory.
//! Reference: the oracle parses all three (`../ruuah/src/terminal/osc.zig`
//!   `clipboard_contents`, `show_desktop_notification`) and its ABI exposes none of it,
//!   same as OSC 8 -- source plus unit tests are the gate.
//! V1 boundaries: OSC 52 QUERIES (`?`) are ignored -- answering means writing bytes
//!   back (the reply seam) and reading a clipboard is a security decision the embedder
//!   has not been asked to make; every selection char is treated as the system
//!   clipboard; base64 is strict (invalid input drops the event, never panics).

use crate::terminal::State;

/// What the embedder is being asked to do. Drained in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// OSC 52: place these bytes on the system clipboard (already base64-decoded).
    ClipboardSet(Vec<u8>),
    /// OSC 9 (body only) or OSC 777;notify;title;body.
    Notify { title: String, body: String },
    /// BEL, outside of any control string.
    Bell,
}

/// Oldest events fall off first past this; see the module card for why.
const MAX_EVENTS: usize = 128;

impl State {
    pub(crate) fn push_event(&mut self, event: Event) {
        if self.events.len() >= MAX_EVENTS {
            self.events.remove(0);
        }
        self.events.push(event);
    }

    /// OSC 52 ; selection ; base64-data. A `?` payload is a read query -- ignored, see
    /// the module card. Invalid base64 drops the whole command, matching the oracle's
    /// decoder which fails the parse rather than delivering garbage.
    pub(crate) fn osc_clipboard(&mut self, params: &[&[u8]]) {
        let Some(payload) = params.get(2) else {
            return;
        };
        if payload == b"?" {
            return;
        }
        if let Some(bytes) = base64_decode(payload) {
            self.push_event(Event::ClipboardSet(bytes));
        }
    }

    /// OSC 9 ; body -- iTerm2's one-argument notification.
    pub(crate) fn osc_notify_9(&mut self, params: &[&[u8]]) {
        let body = join_fields(params.get(1..).unwrap_or_default());
        if !body.is_empty() {
            self.push_event(Event::Notify {
                title: String::new(),
                body,
            });
        }
    }

    /// OSC 777 ; notify ; title ; body -- the rxvt extension every terminal copied.
    pub(crate) fn osc_notify_777(&mut self, params: &[&[u8]]) {
        if params.get(1).copied() != Some(b"notify".as_slice()) {
            return;
        }
        let title = String::from_utf8_lossy(params.get(2).copied().unwrap_or_default()).into_owned();
        let body = join_fields(params.get(3..).unwrap_or_default());
        if !title.is_empty() || !body.is_empty() {
            self.push_event(Event::Notify { title, body });
        }
    }
}

/// Rejoins fields vte split on `;` -- notification bodies may contain them.
fn join_fields(fields: &[&[u8]]) -> String {
    let mut joined = Vec::new();
    for (position, field) in fields.iter().enumerate() {
        if position > 0 {
            joined.push(b';');
        }
        joined.extend_from_slice(field);
    }
    String::from_utf8_lossy(&joined).into_owned()
}

/// Strict RFC 4648 base64: standard alphabet, optional trailing `=` padding, no
/// whitespace. ~20 lines is cheaper than a dependency (GATE 01), and strictness is the
/// point -- a decoder that guesses is how garbage lands on a clipboard.
fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some(u32::from(byte - b'A')),
            b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let trimmed = match input {
        [rest @ .., b'=', b'='] => rest,
        [rest @ .., b'='] => rest,
        rest => rest,
    };
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4 + 3);
    for chunk in trimmed.chunks(4) {
        if chunk.len() == 1 {
            return None; // a lone 6 bits cannot encode a byte
        }
        let mut acc = 0u32;
        for &byte in chunk {
            acc = (acc << 6) | value(byte)?;
        }
        acc <<= 6 * (4 - chunk.len()) as u32;
        let bytes = acc.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::Event;
    use crate::terminal::Terminal;

    #[test]
    fn osc52_decodes_onto_the_event_queue() {
        let mut terminal = Terminal::new(10, 1);
        terminal.write(b"\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(
            terminal.take_events(),
            vec![Event::ClipboardSet(b"hello".to_vec())]
        );
        assert_eq!(terminal.take_events(), vec![], "taking drains");
    }

    #[test]
    fn invalid_base64_and_queries_produce_nothing() {
        let mut terminal = Terminal::new(10, 1);
        terminal.write(b"\x1b]52;c;not!base64\x07\x1b]52;c;?\x07");
        assert_eq!(terminal.take_events(), vec![]);
    }

    #[test]
    fn both_notification_dialects_arrive_with_semicolons_intact() {
        let mut terminal = Terminal::new(10, 1);
        terminal.write(b"\x1b]9;done: a;b\x07\x1b]777;notify;Build;it passed\x07");
        assert_eq!(
            terminal.take_events(),
            vec![
                Event::Notify {
                    title: String::new(),
                    body: "done: a;b".into()
                },
                Event::Notify {
                    title: "Build".into(),
                    body: "it passed".into()
                },
            ]
        );
    }

    #[test]
    fn bel_rings_but_an_osc_terminator_does_not() {
        let mut terminal = Terminal::new(10, 1);
        // The first BEL terminates the OSC; only the second is a real bell.
        terminal.write(b"\x1b]0;title\x07\x07");
        assert_eq!(terminal.take_events(), vec![Event::Bell]);
    }

    #[test]
    fn the_queue_drops_its_oldest_past_the_cap() {
        let mut terminal = Terminal::new(10, 1);
        for _ in 0..200 {
            terminal.write(b"\x07");
        }
        terminal.write(b"\x1b]52;c;eg==\x07");
        let events = terminal.take_events();
        assert_eq!(events.len(), 128, "bounded");
        assert_eq!(
            events.last(),
            Some(&Event::ClipboardSet(b"z".to_vec())),
            "the newest survived; the oldest fell off"
        );
    }
}
