//! S2 compat harness: the integration's marks must survive a shell whose own precmd
//! regenerates PROMPT after ours ran.
//!
//! The live failure (measured in the operator's .zshrc, 2026-07-30): starship's
//! transient-prompt setup registers a precmd that overwrites PROMPT from a saved copy
//! every cycle. The old integration appended its B mark to PROMPT from a hook registered
//! at .zshenv time -- which runs FIRST, so any later-registered hook that rewrites PROMPT
//! silently deletes the input mark and every block goes dark.
//!
//! The test spawns a real interactive zsh through the C surface with a hostile rc that
//! reproduces exactly that shape, then asks `ruuah_host_row_text` with the input filter
//! whether the typed command is still input-marked. Run against the old integration this
//! fails (seen 2026-07-30); the control below proves the marks come from the integration
//! and not from zsh itself.

use std::ffi::CString;
use std::path::PathBuf;
use std::ptr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ruuah_vt_host::{
    RuuahHost, RuuahHostFrame, RuuahHostOptions, RuuahHostResult, ruuah_host_free,
    ruuah_host_poll, ruuah_host_row_text, ruuah_host_send, ruuah_host_spawn,
};

const COLS: u16 = 80;
const ROWS: u16 = 24;
const PATIENCE: Duration = Duration::from_secs(15);

/// Child env is process-global at spawn time, so tests that set it serialize here.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn repo_shell_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../shell")
}

/// A HOME whose .zshrc reproduces the starship shape: a precmd, registered after the
/// integration's (user rc loads after .zshenv), that rewrites PROMPT every cycle.
fn hostile_home() -> PathBuf {
    let home = std::env::temp_dir().join(format!("ruuah-hostile-home-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("temp home");
    std::fs::write(
        home.join(".zshrc"),
        "PROMPT='H> '\n_hostile_restore() { PROMPT='H> ' }\nprecmd_functions+=(_hostile_restore)\n",
    )
    .expect("hostile zshrc");
    home
}

fn empty_frame() -> RuuahHostFrame {
    RuuahHostFrame {
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
    }
}

/// Tolerant variant for polling: `row_text` answers from the last POLLED frame, so this
/// polls first, and before the pump publishes its first frame it returns None ("not
/// yet", not "wrong").
fn try_text_of(host: *mut RuuahHost, row: u16, semantic: u8) -> Option<String> {
    let mut polled = empty_frame();
    if unsafe { ruuah_host_poll(host, &mut polled) } != RuuahHostResult::Success {
        return None;
    }
    let mut len = 0usize;
    let probe = unsafe { ruuah_host_row_text(host, row, semantic, ptr::null_mut(), 0, &mut len) };
    if probe != RuuahHostResult::Success {
        return None;
    }
    let mut buffer = vec![0u8; len];
    let read =
        unsafe { ruuah_host_row_text(host, row, semantic, buffer.as_mut_ptr(), len, &mut len) };
    if read != RuuahHostResult::Success {
        return None;
    }
    String::from_utf8(buffer).ok()
}

fn text_of(host: *mut RuuahHost, row: u16, semantic: u8) -> String {
    let mut len = 0usize;
    assert_eq!(
        unsafe { ruuah_host_row_text(host, row, semantic, ptr::null_mut(), 0, &mut len) },
        RuuahHostResult::Success
    );
    let mut buffer = vec![0u8; len];
    assert_eq!(
        unsafe { ruuah_host_row_text(host, row, semantic, buffer.as_mut_ptr(), len, &mut len) },
        RuuahHostResult::Success
    );
    String::from_utf8(buffer).expect("row text is UTF-8")
}

fn row_classes(host: *mut RuuahHost) -> Vec<u8> {
    let mut polled = empty_frame();
    assert_eq!(
        unsafe { ruuah_host_poll(host, &mut polled) },
        RuuahHostResult::Success
    );
    if polled.row_semantics.is_null() {
        return Vec::new();
    }
    unsafe { std::slice::from_raw_parts(polled.row_semantics, polled.row_count as usize) }.to_vec()
}

fn wait_for<F: FnMut() -> bool>(mut done: F, what: &str) {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

fn send(host: *mut RuuahHost, bytes: &[u8]) {
    assert_eq!(
        unsafe { ruuah_host_send(host, bytes.as_ptr(), bytes.len()) },
        RuuahHostResult::Success
    );
}

fn spawn_zsh(with_integration: bool) -> *mut RuuahHost {
    let shell = repo_shell_dir().canonicalize().expect("shell dir");
    let home = hostile_home();
    // Env mutation is process-global; the lock is held only across spawn, and the child
    // gets its own copy at fork.
    let _guard = ENV_LOCK.lock().expect("env lock");
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::remove_var("RUUAH_USER_ZDOTDIR");
        if with_integration {
            std::env::set_var("ZDOTDIR", shell.join("zdotdir"));
            std::env::set_var("RUUAH_INTEGRATION", shell.join("ruuah-integration.zsh"));
        } else {
            std::env::set_var("ZDOTDIR", &home);
            std::env::remove_var("RUUAH_INTEGRATION");
        }
    }
    let command = CString::new("exec zsh -i").expect("command");
    let options = RuuahHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        command: command.as_ptr(),
        auto_direction: false,
        config: ptr::null(),
    };
    let mut host: *mut RuuahHost = ptr::null_mut();
    assert_eq!(
        unsafe { ruuah_host_spawn(&options, &mut host) },
        RuuahHostResult::Success
    );
    host
}

/// The pair's positive half: under a PROMPT-rewriting precmd, the SECOND prompt cycle
/// (the first is marked before the hostile hook exists in its final order) must still
/// carry the A mark in its prompt and the B mark before the typed command -- which is
/// only possible if the integration re-marks the theme's PROMPT after the theme runs.
#[test]
fn input_marks_survive_a_prompt_rewriting_precmd() {
    let host = spawn_zsh(true);

    // First prompt settles.
    wait_for(
        || try_text_of(host, 0, 255).is_some_and(|text| text.starts_with("H>")),
        "the first prompt",
    );
    // An empty Enter reaches the second cycle, where hook order is final.
    send(host, b"\r");
    wait_for(
        || try_text_of(host, 1, 255).is_some_and(|text| text.starts_with("H>")),
        "the second prompt",
    );
    send(host, b"echo hi\r");
    wait_for(
        || try_text_of(host, 1, 255).is_some_and(|text| text.contains("echo hi")),
        "the typed command to echo",
    );

    let classes = row_classes(host);
    assert_eq!(
        classes.get(1),
        Some(&1),
        "the second prompt row must classify as prompt (A survived the rewrite)"
    );
    // The discriminating read: with B wiped by the hostile precmd, the typed command is
    // prompt-marked and this filter returns "" -- the old integration fails exactly here.
    assert_eq!(
        text_of(host, 1, 2),
        "echo hi",
        "the typed command must be input-marked (B survived the rewrite)"
    );
    assert_eq!(
        text_of(host, 1, 1),
        "H>",
        "the prompt filter keeps only the theme's own cells"
    );
    unsafe { ruuah_host_free(host) };
}

/// The pair's control: the same hostile shell WITHOUT the integration must produce no
/// prompt-classed rows and no input-marked cells. Together with the positive half this
/// proves the marks come from the integration script, not from zsh or the classifier.
#[test]
fn without_the_integration_nothing_is_marked() {
    let host = spawn_zsh(false);

    wait_for(
        || try_text_of(host, 0, 255).is_some_and(|text| text.starts_with("H>")),
        "the first prompt",
    );
    send(host, b"echo hi\r");
    wait_for(
        || try_text_of(host, 0, 255).is_some_and(|text| text.contains("echo hi")),
        "the typed command to echo",
    );

    let classes = row_classes(host);
    assert!(
        classes.iter().all(|&class| class == 0),
        "no integration, no prompt rows -- got {classes:?}"
    );
    assert_eq!(
        text_of(host, 0, 2),
        "",
        "no integration, no input-marked cells"
    );
    unsafe { ruuah_host_free(host) };
}
