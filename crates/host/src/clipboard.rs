//! Purpose: the clipboard path in both directions - bytes to the child, and a selection out.
//! Public surface: `paste`, `paste_text`, `text`, `set_text`.
//! Why this file: both hosts paste, and this was written twice before it was written once. The
//!   split between the two functions here is the load-bearing part: `paste_text` takes the text
//!   as an ARGUMENT, so a gate can exercise the whole encode-and-send path with a fixture and
//!   never read - let alone disturb - the operator's real clipboard. Reading the pasteboard is
//!   the one line that cannot be tested headlessly, and it is now the only one.
//! NOT responsible for: the paste TRANSFORM. `mind2t_vt_pty::paste::encode` owns it and is
//!   measured byte-for-byte against `ghostty_paste_encode`; a second cleaning pass here would
//!   be a second implementation of a rule that already has an oracle.
//! Test strategy: `paste_text` is proven end to end by the Mind2t smoke (a fixture is sent and
//!   must appear on the grid); `text` is a live-tap item by construction.

use crate::session::Session;

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
    let bytes = mind2t_vt_pty::paste::encode(text.as_bytes(), session.bracketed_paste());
    if let Err(error) = session.send(&bytes) {
        eprintln!("mind2t: paste failed: {error:?}");
    }
}

/// Puts `text` on the general pasteboard, replacing whatever was there.
///
/// `clearContents` is not optional and not a courtesy: a pasteboard still holding a previous
/// declaration accepts the write and hands the OLD value to the next reader, so a copy would
/// appear to work and paste something else. The generation it returns is ignored - nothing here
/// races another writer for ownership.
///
/// Empty text is refused. A selection that formats to nothing is a real outcome (a drag across
/// blank cells), and silently emptying the operator's clipboard because they twitched the mouse
/// is worse than doing nothing.
#[cfg(target_os = "macos")]
pub fn set_text(text: &str) -> bool {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::NSString;
    if text.is_empty() {
        return false;
    }
    let pasteboard = NSPasteboard::generalPasteboard();
    unsafe {
        pasteboard.clearContents();
        pasteboard.setString_forType(&NSString::from_str(text), NSPasteboardTypeString)
    }
}

/// The same, through GTK's clipboard, which is the toolkit that already owns the window.
///
/// T3, 2026-08-08. **No new dependency, and that is the answer to "why is the standard path
/// insufficient" - it is not.** `gtk` entered the tree for the key source in T2b and it carries
/// the system clipboard, so reaching for `arboard` or `copypasta` would add a package to do what
/// the toolkit under our own window already does, with its own idea of which display connection
/// to use. GTK's clipboard talks to the same GDK display the window lives on; a third-party
/// crate opens its own, which is a second connection to disagree with the first.
///
/// **`SELECTION_CLIPBOARD`, never `SELECTION_PRIMARY`.** X11 has two, and PRIMARY is the
/// middle-click selection every text widget writes to as you drag. Writing there would replace
/// whatever the operator last highlighted anywhere on their desktop, every time a Mind2t
/// selection changed. CLIPBOARD is the one ctrl+c means.
///
/// **The X11 caveat, stated rather than discovered**: an X11 clipboard is owned by the process
/// that set it, so what we put there vanishes when Mind2t exits unless a clipboard manager is
/// running. `store()` asks the manager to keep it and is a no-op when there is none - which is
/// why it is called rather than assumed, and why this cannot promise more than it does.
#[cfg(target_os = "linux")]
pub fn set_text(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
    clipboard.set_text(text);
    clipboard.store();
    true
}

/// No clipboard reached for on other platforms yet, and answering `false` says so rather than
/// reporting a copy that never happened.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn set_text(text: &str) -> bool {
    let _ = text;
    false
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

/// The same, through GTK. `None` when the clipboard holds something that is not text.
///
/// **`wait_for_text` blocks and PUMPS THE MAIN LOOP while it waits**, because on X11 fetching the
/// clipboard is a round trip to whichever process owns it. Two consequences worth knowing before
/// this is called from anywhere new:
/// - it must be called on the main thread, which every caller here already is;
/// - it is re-entrant. Other GTK callbacks can run inside this call, so a caller holding a
///   `RefCell` borrow across it would panic on the re-entry rather than here. The paste path
///   reads the clipboard BEFORE it borrows the canvas, and that ordering is the reason.
///
/// A dead or unresponsive owner makes this hang until GTK's own timeout, which is the cost of
/// the X11 model and not something a wrapper can fix.
#[cfg(target_os = "linux")]
pub fn text() -> Option<String> {
    gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD)
        .wait_for_text()
        .map(|value| value.to_string())
}

/// No clipboard reached for on other platforms yet, and saying so out loud beats a paste that
/// silently does nothing on the day Mind2t is built for one.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn text() -> Option<String> {
    None
}
