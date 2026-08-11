//! The gate for a defect that has now come back three times: a pane that opens at `/`.
//!
//! History, from the repo's own record. Fixed in the C ABI host 2026-07-30 after the operator
//! reported "cannot cd into Desktop" and a pane opening on the sealed read-only root, which
//! starship dresses in a padlock and which reads as a permission block. Never carried across to
//! the second host. Hit again 2026-08-08. Live again 2026-08-11.
//!
//! Three point fixes and no gate is why. `CLAUDE.md` says it outright: "the durable fix is a gate
//! asserting a spawned pane's cwd, not a fourth point fix." This is that gate, and it lives in
//! `crates/host` rather than in either host's own suite BECAUSE the defect's whole shape is that
//! one host was fixed and the other was not. Every host reaches the pty through
//! `mind2t_host_spawn`, so an assertion here covers the Swift host, the Tauri host, and whatever
//! replaces them.
//!
//! It asks the child, rather than reading the parent's intent. `Command::current_dir` is what the
//! three point fixes each set correctly at some call site while the live pane still opened
//! somewhere else, so asserting on the `Command` would have passed every time the bug shipped.
//! `pwd` down a real pty is the only answer that cannot be wrong.

use std::ffi::CString;
use std::path::PathBuf;
use std::ptr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mind2t_vt_host::{
    Mind2tHost, Mind2tHostFrame, Mind2tHostOptions, Mind2tHostResult, mind2t_host_free,
    mind2t_host_poll, mind2t_host_row_text, mind2t_host_send, mind2t_host_spawn,
};

const COLS: u16 = 80;
const ROWS: u16 = 24;
const PATIENCE: Duration = Duration::from_secs(20);

/// Child env is process-global at spawn time, so tests that set it serialize here.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A quiet HOME: an empty rc, so the prompt is ours and `pwd` output is not buried in a
/// theme. The point of the test is a directory, not a shell configuration.
fn quiet_home(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("mind2t-cwd-gate-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("temp home");
    std::fs::write(home.join(".zshrc"), "PROMPT='> '\n").expect("quiet zshrc");
    std::fs::write(home.join(".zprofile"), "").expect("quiet zprofile");
    // Canonicalised because macOS hands out `/var/...` while the real path is `/private/var/...`,
    // and `pwd` prints the resolved one. Comparing the two forms is a false red that costs an
    // hour the first time and every time somebody forgets.
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

/// Every row of the last polled frame, joined. `row_text` answers from the POLLED frame, so
/// this polls first; before the pump publishes anything it yields an empty string, which reads
/// as "not yet" to the waiter rather than as a wrong answer.
fn screen(host: *mut Mind2tHost) -> String {
    let mut polled = empty_frame();
    if unsafe { mind2t_host_poll(host, &mut polled) } != Mind2tHostResult::Success {
        return String::new();
    }
    let mut out = String::new();
    for row in 0..ROWS {
        let mut len = 0usize;
        if unsafe { mind2t_host_row_text(host, row, 255, ptr::null_mut(), 0, &mut len) }
            != Mind2tHostResult::Success
        {
            continue;
        }
        let mut buffer = vec![0u8; len];
        if unsafe { mind2t_host_row_text(host, row, 255, buffer.as_mut_ptr(), len, &mut len) }
            == Mind2tHostResult::Success
            && let Ok(text) = String::from_utf8(buffer)
        {
            out.push_str(&text);
            out.push('\n');
        }
    }
    out
}

/// Spawns the DEFAULT pane - `command: null` - which is the branch every host takes when the
/// operator opens a window and the one all three recurrences were in. An explicit command takes
/// the other branch and keeps the caller's cwd on purpose, so testing that branch here would
/// assert the opposite of the property under test.
fn spawn_default_pane(home: &PathBuf, explicit_cwd: Option<&PathBuf>) -> *mut Mind2tHost {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let cwd_string = explicit_cwd.map(|path| CString::new(path.to_str().expect("utf-8 cwd")).expect("cwd"));
    unsafe {
        std::env::set_var("HOME", home);
        std::env::set_var("ZDOTDIR", home);
        std::env::set_var("SHELL", "/bin/zsh");
        std::env::remove_var("MIND2T_INTEGRATION");
        std::env::remove_var("MIND2T_USER_ZDOTDIR");
        // Same reason `shell_integration.rs` scrubs it, measured the same day: `/etc/zshrc:74`
        // sources `/etc/zshrc_$TERM_PROGRAM`, so a suite run from Terminal.app loads Apple's own
        // integration into a shell this test believes it configured alone.
        std::env::remove_var("TERM_PROGRAM");
    }
    let options = Mind2tHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        command: ptr::null(),
        auto_direction: false,
        config: ptr::null(),
        cwd: cwd_string.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
    };
    let mut host: *mut Mind2tHost = ptr::null_mut();
    assert_eq!(
        unsafe { mind2t_host_spawn(&options, &mut host) },
        Mind2tHostResult::Success
    );
    host
}

/// The same rows with the row breaks removed, for matching rather than for reading.
///
/// A pty WRAPS: at 80 columns a temp-directory path lands as `...mind2t-cwd-gate` on one row and
/// `-home-65607]` on the next, and a substring search against the row-broken form finds neither
/// half. Measured on this test's first run, which reported a timeout while the terminal was
/// holding the correct answer in two pieces. Widening the grid would hide the wrap rather than
/// handle it, and a terminal test that only works when nothing wraps is not much of one.
fn joined(host: *mut Mind2tHost) -> String {
    screen(host).replace('\n', "")
}

/// Asks the child where it is and waits for the answer to appear on screen.
fn child_reports_cwd(host: *mut Mind2tHost, expected: &PathBuf) -> bool {
    let needle = expected.to_str().expect("utf-8 path").to_string();
    // A marker around the answer, because the EXPECTED PATH IS ALSO IN THE PROMPT for most
    // themes and in the shell's own startup noise. Without it this test passes on a pane that
    // opened somewhere else entirely and merely printed the string once.
    unsafe {
        let line = b"printf 'CWDGATE[%s]\\n' \"$PWD\"\r";
        assert_eq!(
            mind2t_host_send(host, line.as_ptr(), line.len()),
            Mind2tHostResult::Success
        );
    }
    // Not `wait_for`: a bare timeout here says only "nothing appeared", and the four things that
    // can cause that (no shell, no prompt, no echo, wrong row filter) are indistinguishable
    // without the screen. The deadline PRINTS what the terminal actually held.
    // `CWDGATE[/`, not `CWDGATE[`. The command line is ECHOED by the pty, so the literal
    // `CWDGATE[%s]` is on screen before the child has run anything at all - the first version of
    // this waited on the bare marker, matched its own echo, and then declared a timeout while the
    // right answer sat two rows below. Only the expansion begins with a slash.
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        let seen = joined(host);
        if seen.contains("CWDGATE[/") {
            return seen.contains(&format!("CWDGATE[{needle}]"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "the child never reported $PWD. The screen held:\n{}\n(end of screen)",
        screen(host)
    );
}

/// THE GATE. A default pane opens at HOME.
///
/// Seen red before it was believed, on 2026-08-11, by removing the `current_dir(home)` branch
/// from `build_command`: the child then reported `CWDGATE[/]` - the sealed read-only root, which
/// is the exact live symptom, reproduced from a test rather than from an operator's screenshot.
#[test]
fn a_default_pane_opens_at_home() {
    let home = quiet_home("home");
    let host = spawn_default_pane(&home, None);
    let landed = child_reports_cwd(host, &home);
    unsafe { mind2t_host_free(host) };
    assert!(
        landed,
        "a default pane must open at $HOME ({}), and this is the fourth time it did not",
        home.display()
    );
}

/// THE CONTROL, and it is the half that makes the gate above mean anything.
///
/// It proves the test observes the CHILD's directory rather than always finding HOME: given an
/// explicit `cwd`, the child must report THAT instead. Without this, `a_default_pane_opens_at_home`
/// would still pass if `child_reports_cwd` were broken into always answering true, and a gate
/// that cannot fail is indistinguishable from one that passes.
#[test]
fn an_explicit_cwd_outranks_the_home_default() {
    let home = quiet_home("override");
    let elsewhere = std::env::temp_dir()
        .join(format!("mind2t-cwd-gate-elsewhere-{}", std::process::id()))
        .canonicalize()
        .unwrap_or_else(|_| {
            let path = std::env::temp_dir()
                .join(format!("mind2t-cwd-gate-elsewhere-{}", std::process::id()));
            std::fs::create_dir_all(&path).expect("elsewhere");
            path.canonicalize().expect("canonical elsewhere")
        });
    let host = spawn_default_pane(&home, Some(&elsewhere));
    let landed = child_reports_cwd(host, &elsewhere);
    unsafe { mind2t_host_free(host) };
    assert!(
        landed,
        "an explicit cwd ({}) must outrank the HOME default ({})",
        elsewhere.display(),
        home.display()
    );
}
