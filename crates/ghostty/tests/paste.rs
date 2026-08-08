//! The paste encoder, measured against the oracle's own.
//!
//! `ghostty_paste_encode` is the reference implementation of the transform
//! `crates/pty/src/paste.rs` performs before a paste reaches the pty. Byte-for-byte
//! equality over inputs that exercise every rule -- the strip set, the fenceposts, the
//! newline fold -- is what lets the host's paste path claim the same drop-in fidelity
//! as the grid. The control at the bottom proves the comparison can fail: an encoder
//! with the bracketed flag inverted must disagree on every fixture that has a fence
//! or a newline.

use mind2t_vt_ghostty::sys;

/// Every rule from `src/input/paste.zig`, plus the shapes that mix them.
const FIXTURES: &[&[u8]] = &[
    b"",
    b"hello",
    b"line one\nline two",
    b"crlf\r\nend",
    b"lone\rcarriage",
    b"evil\x1b[201~rm -rf /",
    b"\x00\x03\x04\x05\x08\x0f\x11\x12\x13\x15\x16\x17\x1a\x1b\x1c\x7f",
    b"ctrl-c\x03 in the middle",
    "שלום\nעולם".as_bytes(),
    b"\n",
    b"\x1b",
    b"tab\tand bell\x07 survive",
];

/// Runs the oracle's encoder: data is modified in place, the output buffer is sized by
/// a first call that is allowed to come back OUT_OF_SPACE.
fn oracle_encode(data: &[u8], bracketed: bool) -> Vec<u8> {
    let mut scratch = data.to_vec();
    let mut needed = 0usize;
    let first = unsafe {
        sys::ghostty_paste_encode(
            scratch.as_mut_ptr().cast(),
            scratch.len(),
            bracketed,
            std::ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    if first != sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        assert_eq!(first, sys::GhosttyResult_GHOSTTY_SUCCESS, "sizing call");
        assert_eq!(needed, 0, "success with no buffer must have written nothing");
        return Vec::new();
    }

    // The sizing call already stripped in place; encode from a fresh copy so the
    // measured transform is the whole transform.
    scratch.copy_from_slice(data);
    let mut out = vec![0u8; needed];
    let mut written = 0usize;
    let code = unsafe {
        sys::ghostty_paste_encode(
            scratch.as_mut_ptr().cast(),
            scratch.len(),
            bracketed,
            out.as_mut_ptr().cast(),
            out.len(),
            &mut written,
        )
    };
    assert_eq!(code, sys::GhosttyResult_GHOSTTY_SUCCESS, "encoding call");
    out.truncate(written);
    out
}

#[test]
fn the_encoder_matches_the_oracle_byte_for_byte() {
    for &fixture in FIXTURES {
        for bracketed in [false, true] {
            let ours = mind2t_vt_pty::paste::encode(fixture, bracketed);
            let oracle = oracle_encode(fixture, bracketed);
            assert_eq!(
                ours, oracle,
                "fixture {:?} bracketed={bracketed}: ours vs oracle",
                String::from_utf8_lossy(fixture)
            );
        }
    }
}

/// The comparison can fail: flipping the bracketed flag must disagree wherever the
/// fence or the newline fold is observable. If this passes, the harness above is
/// comparing nothing.
#[test]
fn an_encoder_with_the_flag_inverted_is_caught() {
    let mut disagreements = 0;
    for &fixture in FIXTURES {
        for bracketed in [false, true] {
            let wrong = mind2t_vt_pty::paste::encode(fixture, !bracketed);
            if wrong != oracle_encode(fixture, bracketed) {
                disagreements += 1;
            }
        }
    }
    assert!(disagreements >= FIXTURES.len(), "{disagreements} disagreements");
}
