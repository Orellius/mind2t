//! Purpose: turn clipboard bytes into the bytes a paste writes to the pty.
//! Public surface: `encode`.
//! Why this file: pasting is an INPUT encoding, like the key encoder -- the core never
//!   sees a paste as anything but ordinary bytes, so the transform lives with the pty,
//!   not the core. It is pure so the differential harness can compare it byte-for-byte
//!   against `ghostty_paste_encode`.
//! NOT responsible for: deciding whether bracketed paste is on (the core tracks mode
//!   2004; the host asks it), or writing to the pty (`host.rs`).
//! Test strategy: `crates/ghostty/tests/paste.rs` measures this against the oracle's own
//!   encoder over inputs covering every rule; unit tests here pin the properties that
//!   motivated each rule.

/// The byte values every text-insertion method replaces with a space, copied from
/// xterm via Ghostty (`src/input/paste.zig`). Stripping happens REGARDLESS of
/// bracketed paste mode: ESC in the set is what makes a pasted `ESC[201~` unable to
/// close the fence and inject commands, and the VINTR/VSUSP family keeps a paste from
/// signalling the foreground process.
const STRIP: &[u8] = &[
    0x00, // NUL
    0x08, // BS
    0x05, // ENQ
    0x04, // EOT
    0x1B, // ESC
    0x7F, // DEL
    0x03, // VINTR (Ctrl+C)
    0x1C, // VQUIT (Ctrl+\)
    0x15, // VKILL (Ctrl+U)
    0x1A, // VSUSP (Ctrl+Z)
    0x11, // VSTART (Ctrl+Q)
    0x13, // VSTOP (Ctrl+S)
    0x17, // VWERASE (Ctrl+W)
    0x16, // VLNEXT (Ctrl+V)
    0x12, // VREPRINT (Ctrl+R)
    0x0F, // VDISCARD (Ctrl+O)
];

/// Encodes paste data for the pty.
///
/// Unsafe bytes become spaces first. Then bracketed mode wraps the result in the
/// `ESC[200~` / `ESC[201~` fenceposts and changes nothing else -- newlines survive,
/// because the fence is the child's promise to treat them as data. Unbracketed,
/// every `\n` becomes `\r` (so `\r\n` becomes `\r\r`, which matches xterm), because
/// a raw `\n` on a canonical-mode pty is an immediate Enter per line.
pub fn encode(data: &[u8], bracketed: bool) -> Vec<u8> {
    let cleaned = data.iter().map(|&byte| {
        if STRIP.contains(&byte) {
            b' '
        } else if !bracketed && byte == b'\n' {
            b'\r'
        } else {
            byte
        }
    });

    if bracketed {
        let mut out = Vec::with_capacity(data.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        out.extend(cleaned);
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        cleaned.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::encode;

    /// The attack the fence exists to stop: a paste that carries its own closing fence
    /// would end bracketed mode early and hand the rest to the shell as typed input.
    /// ESC -> space means the fence text survives only as inert printables.
    #[test]
    fn a_pasted_close_fence_cannot_close_the_fence() {
        let out = encode(b"x\x1b[201~rm -rf /", true);
        let interior = &out[6..out.len() - 6];
        assert!(!interior.windows(2).any(|w| w == b"\x1b["), "{interior:?}");
        assert_eq!(interior, b"x [201~rm -rf /");
    }

    #[test]
    fn bracketed_keeps_newlines_inside_the_fence() {
        assert_eq!(encode(b"a\nb", true), b"\x1b[200~a\nb\x1b[201~");
    }

    #[test]
    fn unbracketed_folds_newlines_to_carriage_returns() {
        // CRLF becomes CR CR, which is what xterm does.
        assert_eq!(encode(b"a\r\nb\n", false), b"a\r\rb\r");
    }

    #[test]
    fn stripping_applies_without_the_fence_too() {
        assert_eq!(encode(b"a\x03b\x00c", false), b"a b c");
    }
}
