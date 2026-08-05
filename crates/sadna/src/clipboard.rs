//! Purpose: the paste path - clipboard bytes to the child, fenced when the child asked.
//! Public surface: `paste`, `paste_text`, `text`.
//! Why this file: both hosts paste, and this was written twice before it was written once. The
//!   split between the two functions here is the load-bearing part: `paste_text` takes the text
//!   as an ARGUMENT, so a gate can exercise the whole encode-and-send path with a fixture and
//!   never read - let alone disturb - the operator's real clipboard. Reading the pasteboard is
//!   the one line that cannot be tested headlessly, and it is now the only one.
//! NOT responsible for: the paste TRANSFORM. `ruuah_vt_pty::paste::encode` owns it and is
//!   measured byte-for-byte against `ghostty_paste_encode`; a second cleaning pass here would
//!   be a second implementation of a rule that already has an oracle.
//! Test strategy: `paste_text` is proven end to end by the Sadna smoke (a fixture is sent and
//!   must appear on the grid); `text` is a live-tap item by construction.

use ruuah_vt_host::session::Session;

/// Sends the clipboard to the child, fenced when the child asked for fences.
///
/// The fences are not cosmetic: without DEC 2004 bracketing, a shell receiving a multi-line
/// paste executes each line as it arrives instead of taking the whole thing as one edit, which
/// is how a pasted script runs half of itself. Whether to fence is the CHILD's decision, read
/// from the frame rather than configured here.
pub fn paste(session: &Session) {
    let Some(text) = text() else {
        return;
    };
    paste_text(session, &text);
}

/// Sends `text` to the child as a paste. The half a gate can drive.
pub fn paste_text(session: &Session, text: &str) {
    let bytes = ruuah_vt_pty::paste::encode(text.as_bytes(), session.bracketed_paste());
    if let Err(error) = session.send(&bytes) {
        eprintln!("sadna: paste failed: {error:?}");
    }
}

/// The general pasteboard's string, or `None` when it holds something else (an image, a file).
///
/// Reached through `objc2-app-kit`, which is already in the tree under wry and muda, so this
/// costs a direct dependency and no new package.
#[cfg(target_os = "macos")]
pub fn text() -> Option<String> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    let pasteboard = NSPasteboard::generalPasteboard();
    let value = unsafe { pasteboard.stringForType(NSPasteboardTypeString) };
    value.map(|string| string.to_string())
}

/// No clipboard reached for on other platforms yet, and saying so out loud beats a paste that
/// silently does nothing on the day Sadna is built for one (B6).
#[cfg(not(target_os = "macos"))]
pub fn text() -> Option<String> {
    None
}
