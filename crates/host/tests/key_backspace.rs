//! Backspace, end to end: the key event in, the erased line out.
//!
//! Orel, 2026-08-11: "why using backspace is literally spacing the text?" That symptom has a
//! specific shape. A shell erases the character under the cursor by sending `\b \b` - move left,
//! overwrite with a space, move left again - so a terminal that gets the LAST `\b` wrong leaves
//! the space it just wrote sitting where the character was. "Spacing the text" is exactly what a
//! broken `\b \b` looks like, and it is invisible to any test that only checks the encoder.
//!
//! So this drives the whole chain rather than the table: `mind2t_host_key` -> the encoder ->
//! a real pty -> a real zsh's line editor -> the core -> the grid. Every layer in between has
//! its own unit tests and all of them pass, which is why the defect was reported from a screen
//! and not from a suite.
//!
//! It lives in `crates/host` because both hosts share this path. The Swift host maps macOS
//! keyCode 51 to key id 53 in `KeyMap.swift`, and `Key::ALL` is in declaration order, so id 53
//! is `Key::Backspace` for the Tauri host too. One gate covers both.

use std::ffi::CString;
use std::path::PathBuf;
use std::ptr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mind2t_vt_host::{
    Mind2tHost, Mind2tHostFrame, Mind2tHostOptions, Mind2tHostResult, mind2t_host_free,
    mind2t_host_key, mind2t_host_poll, mind2t_host_row_text, mind2t_host_send, mind2t_host_spawn,
};

const COLS: u16 = 80;
const ROWS: u16 = 24;
const PATIENCE: Duration = Duration::from_secs(20);

/// `Key::ALL` is in C declaration order by construction and `mind2t_host_key` indexes it, so this
/// is `Key::Backspace`. It is also what `KeyMap.swift` maps macOS keyCode 51 onto - the two agree
/// by construction rather than by coincidence, and the keycode parity test is what keeps them so.
const KEY_BACKSPACE: u32 = 53;
const ACTION_PRESS: u32 = 1;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn quiet_home(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("mind2t-bs-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("temp home");
    // A one-character prompt with no theme, so the row under test is the typed line and nothing
    // else. A themed prompt would put the answer at an offset this test would then hardcode.
    std::fs::write(home.join(".zshrc"), "PROMPT='%% '\n").expect("quiet zshrc");
    std::fs::write(home.join(".zprofile"), "").expect("quiet zprofile");
    home.canonicalize().expect("canonical home")
}

fn empty_frame() -> Mind2tHostFrame {
    Mind2tHostFrame {
        pixels: ptr::null(),
        width: 0,
        height: 0,
        generation: 0,
        drew: false,
        child_exited: false,
        background: [0; 4],
        row_semantics: ptr::null(),
        row_count: 0,
        viewport_offset: 0,
        cursor_col: 0,
        cursor_row: 0,
        cursor_visible: false,
    }
}

/// Row 0 of the last polled frame, which is where the prompt and the typed line live.
fn row0(host: *mut Mind2tHost) -> String {
    let mut polled = empty_frame();
    if unsafe { mind2t_host_poll(host, &mut polled) } != Mind2tHostResult::Success {
        return String::new();
    }
    let mut len = 0usize;
    if unsafe { mind2t_host_row_text(host, 0, 255, ptr::null_mut(), 0, &mut len) }
        != Mind2tHostResult::Success
    {
        return String::new();
    }
    let mut buffer = vec![0u8; len];
    if unsafe { mind2t_host_row_text(host, 0, 255, buffer.as_mut_ptr(), len, &mut len) }
        != Mind2tHostResult::Success
    {
        return String::new();
    }
    String::from_utf8(buffer).unwrap_or_default()
}

fn spawn_quiet_shell(home: &PathBuf) -> *mut Mind2tHost {
    spawn_with_direction(home, false)
}

fn spawn_with_direction(home: &PathBuf, auto_direction: bool) -> *mut Mind2tHost {
    let _guard = ENV_LOCK.lock().expect("env lock");
    unsafe {
        std::env::set_var("HOME", home);
        std::env::set_var("ZDOTDIR", home);
        std::env::remove_var("MIND2T_INTEGRATION");
        // `/etc/zshrc:74` sources `/etc/zshrc_$TERM_PROGRAM`, so a suite run from Terminal.app
        // loads Apple's own integration into a shell this test believes it configured alone.
        std::env::remove_var("TERM_PROGRAM");
    }
    let command = CString::new("exec zsh -i").expect("command");
    let options = Mind2tHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        command: command.as_ptr(),
        auto_direction,
        config: ptr::null(),
        cwd: ptr::null(),
    };
    let mut host: *mut Mind2tHost = ptr::null_mut();
    assert_eq!(
        unsafe { mind2t_host_spawn(&options, &mut host) },
        Mind2tHostResult::Success
    );
    host
}

fn wait_until<F: FnMut(&str) -> bool>(host: *mut Mind2tHost, mut done: F, what: &str) -> String {
    let deadline = Instant::now() + PATIENCE;
    let mut seen = String::new();
    while Instant::now() < deadline {
        seen = row0(host);
        if done(&seen) {
            return seen;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}. Row 0 held: {seen:?}");
}

/// A real Backspace PRESS, shaped the way a host sends one.
///
/// `text` is `\x7f` and `unshifted_codepoint` is `0x7f` because that is what macOS puts in
/// `NSEvent.characters` for this key and what the encoder's own unit test feeds it. Passing an
/// empty text here would test a key event no host actually produces.
fn press_backspace(host: *mut Mind2tHost) {
    let text = [0x7fu8];
    assert_eq!(
        unsafe {
            mind2t_host_key(
                host,
                ACTION_PRESS,
                KEY_BACKSPACE,
                0,
                0,
                text.as_ptr(),
                text.len(),
                0x7f,
            )
        },
        Mind2tHostResult::Success
    );
}

/// THE GATE. Backspace erases; it does not leave a space behind.
///
/// The assertion is on the FULL row rather than on "does it contain abc", because `abc` is a
/// substring of `abcd` and of `abc d` - the two states this test exists to tell apart. A
/// containment check here would pass on precisely the symptom that was reported.
#[test]
fn backspace_erases_the_character_rather_than_blanking_it() {
    let home = quiet_home("erase");
    let host = spawn_quiet_shell(&home);

    wait_until(host, |row| row.starts_with('%'), "the first prompt");
    unsafe {
        let typed = b"abcd";
        assert_eq!(
            mind2t_host_send(host, typed.as_ptr(), typed.len()),
            Mind2tHostResult::Success
        );
    }
    wait_until(host, |row| row.contains("abcd"), "the typed line");

    press_backspace(host);
    // The trailing `d` must go, and nothing may take its place. `trim_end` is deliberate and
    // narrow: a terminal row is padded to its width, so trailing blanks past the end of the line
    // are the grid's own padding rather than the artefact under test. A space BETWEEN the prompt
    // and the end of the text would survive this trim, which is the case that matters.
    let settled = wait_until(
        host,
        |row| !row.contains("abcd"),
        "the character to be erased",
    );
    let line = settled.trim_end().to_string();
    unsafe { mind2t_host_free(host) };

    assert!(
        line.ends_with("abc"),
        "backspace must erase the character, leaving the line ending in `abc`. \
         A line ending in a blank is the reported symptom - backspace spacing the text \
         instead of deleting it. Row 0 was {settled:?}"
    );
}

/// The same erase on an RTL line, with auto-direction on - the configuration this terminal
/// exists for and the one no other suite covers here.
///
/// A Hebrew word is stored in logical order and DISPLAYED reordered, so "the last character
/// typed" and "the rightmost cell" are different cells. That is the one place where an erase can
/// blank the wrong cell while every ASCII test stays green, and it is the reading of "backspace
/// is spacing the text" that this codebase could plausibly get wrong on its own rather than
/// inheriting from a shell.
///
/// It asserts on the LOGICAL row text, which is what `row_text` returns: the character typed
/// last must be gone from the string. Reordering is the renderer's job and has its own pixel
/// tests; conflating the two here would make this fail for a reason it is not about.
#[test]
fn backspace_on_an_rtl_line_removes_the_last_typed_character() {
    let home = quiet_home("rtl");
    let host = spawn_with_direction(&home, true);

    wait_until(host, |row| row.starts_with('%'), "the first prompt");
    // Four Hebrew letters. The last one is what backspace must remove, and it is the one that
    // renders LEFTMOST rather than rightmost - which is the whole point of the case.
    let typed = "שלום";
    unsafe {
        assert_eq!(
            mind2t_host_send(host, typed.as_bytes().as_ptr(), typed.len()),
            Mind2tHostResult::Success
        );
    }
    wait_until(host, |row| row.contains(typed), "the typed Hebrew line");

    press_backspace(host);
    let settled = wait_until(
        host,
        |row| !row.contains(typed),
        "the Hebrew character to be erased",
    );
    let line = settled.trim_end().to_string();
    unsafe { mind2t_host_free(host) };

    assert!(
        line.ends_with("שלו"),
        "backspace on an RTL line must remove the last character typed, leaving `שלו`. \
         Row 0 was {settled:?}"
    );
}

/// THE CONTROL, and it is what makes the gate above mean anything.
///
/// It proves the test can SEE a character at the end of the line: without pressing backspace,
/// the same assertion must find `abcd` and therefore NOT end in `abc`. If `row_text` were
/// trimming the last cell, or the row were always empty, the gate would pass vacuously and
/// report success on a terminal where backspace does nothing at all.
#[test]
fn the_untouched_line_still_ends_in_the_character_backspace_would_remove() {
    let home = quiet_home("control");
    let host = spawn_quiet_shell(&home);

    wait_until(host, |row| row.starts_with('%'), "the first prompt");
    unsafe {
        let typed = b"abcd";
        assert_eq!(
            mind2t_host_send(host, typed.as_ptr(), typed.len()),
            Mind2tHostResult::Success
        );
    }
    let settled = wait_until(host, |row| row.contains("abcd"), "the typed line");
    let line = settled.trim_end().to_string();
    unsafe { mind2t_host_free(host) };

    assert!(
        line.ends_with("abcd") && !line.ends_with("abc "),
        "with no backspace the line must still carry its last character. Row 0 was {settled:?}"
    );
}
