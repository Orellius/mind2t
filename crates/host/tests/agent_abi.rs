//! The agent surface, driven through the C ABI rather than through Rust.
//!
//! `agent.rs` has been correct and unreachable since 2026-08-11: zero exports, so the shipped
//! app could not launch an agent into a pane at all. Its own unit tests prove the registry and
//! the guard as Rust; they cannot prove that an embedder can reach either, which is the whole
//! defect this file closes. Everything here goes through the same entry points Swift calls.
//!
//! The launch tests shadow a registry binary on `PATH` with a fake rather than starting a real
//! agent. That is not a weaker test - it is a STRONGER one, and it is the only version that can
//! run in an automated suite at all: `launch.rs`'s real-agent test is `#[ignore]`d precisely
//! because a suite must not start authenticated agent processes on someone's machine every run.
//! A fake on `PATH` still exercises registry lookup, the `PATH` walk, the exec-bit check,
//! `launch::dress`, the pty, the pump and the renderer - everything except the vendor's own
//! binary, which is not ours to test.

use std::ffi::{CStr, CString};
use std::path::PathBuf;
use std::ptr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use mind2t_vt_host::{
    Mind2tAgentInfo, Mind2tHost, Mind2tHostFrame, Mind2tHostOptions, Mind2tHostResult,
    mind2t_agent_count, mind2t_agent_info, mind2t_agent_resolve, mind2t_agent_screen,
    mind2t_host_free, mind2t_host_poll, mind2t_host_row_text, mind2t_host_spawn_agent,
};

/// Wide enough that the marker line below does not wrap for a temp path of any plausible
/// length. The screen is joined anyway, so a wrap would not break the test - it would only
/// make a failure message unreadable.
const COLS: u16 = 200;
const ROWS: u16 = 24;
const PATIENCE: Duration = Duration::from_secs(20);

/// `PATH` and `HOME` are PROCESS-global and cargo runs these tests in parallel threads, so a
/// test that sets either sets it for every sibling for as long as it holds. Measured in
/// `agent.rs` on 2026-08-06, where exactly this raced and produced a red suite that went green
/// on re-run with nothing wrong in the product. Every test that touches the environment takes
/// this lock, and a re-run is never the fix.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn argv(words: &[&str]) -> Vec<CString> {
    words
        .iter()
        .map(|word| CString::new(*word).expect("no interior NUL"))
        .collect()
}

/// Screens an argv through the C surface, answering (result, flag, position).
fn screen(words: &[&str]) -> (Mind2tHostResult, String, u32) {
    let owned = argv(words);
    let pointers: Vec<*const std::ffi::c_char> =
        owned.iter().map(|word| word.as_ptr()).collect();
    let (mut at, mut len) = (0u32, 0usize);

    // Sized first, filled second - the two-call shape every string out-param in this header
    // uses, and the half that would hide a truncation bug if it were skipped.
    let sizing = unsafe {
        mind2t_agent_screen(
            pointers.as_ptr(),
            pointers.len(),
            &mut at,
            ptr::null_mut(),
            0,
            &mut len,
        )
    };
    let mut buffer = vec![0u8; len];
    let verdict = unsafe {
        mind2t_agent_screen(
            pointers.as_ptr(),
            pointers.len(),
            &mut at,
            if len == 0 {
                ptr::null_mut()
            } else {
                buffer.as_mut_ptr()
            },
            len,
            &mut len,
        )
    };
    assert_eq!(sizing, verdict, "sizing and filling disagreed about {words:?}");
    (
        verdict,
        String::from_utf8(buffer).expect("the guard's flags are ASCII"),
        at,
    )
}

/// Every registry entry is reachable by the id the embedder will name it by, with the two
/// fields a menu cannot be built without.
#[test]
fn the_registry_is_readable_through_the_c_surface() {
    let count = mind2t_agent_count();
    assert!(count > 0, "the C surface reports an empty registry");

    let mut typed_after_launch = 0;
    for index in 0..count {
        let mut info = Mind2tAgentInfo {
            id: ptr::null(),
            name: ptr::null(),
            install_hint: ptr::null(),
            spawn_grace_ms: 0,
            type_after_launch: false,
        };
        assert_eq!(
            unsafe { mind2t_agent_info(index, &mut info) },
            Mind2tHostResult::Success
        );
        let id = unsafe { CStr::from_ptr(info.id) }.to_str().expect("utf-8 id");
        let name = unsafe { CStr::from_ptr(info.name) }.to_str().expect("utf-8 name");
        let hint = unsafe { CStr::from_ptr(info.install_hint) }
            .to_str()
            .expect("utf-8 hint");
        assert!(!id.is_empty() && !name.is_empty() && !hint.is_empty(), "{id}: a blank field");
        // Zero would mean "a launch has no grace at all", which is not a value any entry
        // holds and would make every slow agent read as failed the instant it started.
        assert!(info.spawn_grace_ms > 0, "{id}: no spawn grace");
        if info.type_after_launch {
            typed_after_launch += 1;
        }
    }
    // Both prompt strategies survive the trip. Without this the flag could be hardwired false
    // and every test above would still pass, while the embedder silently handed a prompt as
    // argv to the agents that take it as a filename.
    assert!(typed_after_launch > 0, "no agent reports type_after_launch");
    assert!(
        typed_after_launch < count,
        "every agent reports type_after_launch, so the flag is not being read"
    );

    let mut past_the_end = Mind2tAgentInfo {
        id: ptr::null(),
        name: ptr::null(),
        install_hint: ptr::null(),
        spawn_grace_ms: 0,
        type_after_launch: false,
    };
    assert_eq!(
        unsafe { mind2t_agent_info(count, &mut past_the_end) },
        Mind2tHostResult::InvalidValue,
        "an out-of-range index was answered instead of refused"
    );
}

/// The guard, in BOTH directions, because either alone is worthless: a function that refuses
/// every argv passes the first, and one that refuses nothing passes the second.
#[test]
fn the_guard_answers_both_directions_through_the_c_surface() {
    let bypasses: &[&[&str]] = &[
        &["--yolo"],
        &["--dangerously-skip-permissions"],
        &["--auto"],
        &["-a", "never"],
        &["--ask-for-approval", "never"],
        // Buried mid-argv: a guard that only reads the first word is one that anything gets
        // past by putting a flag in front of it.
        &["--model", "sonnet", "--yolo", "--verbose"],
    ];
    for case in bypasses {
        let (verdict, flag, _) = screen(case);
        assert_eq!(verdict, Mind2tHostResult::Refused, "{case:?} was allowed through");
        assert!(!flag.is_empty(), "{case:?} was refused without naming the flag");
    }

    // Every one of these LOOKS like a bypass and is not. A guard that refuses them is one the
    // operator learns to work around, which is how a guard becomes worse than none.
    let safe: &[&[&str]] = &[
        &["--autosave"],
        &["--auto-detect"],
        &["--yolonaut"],
        &["--model", "never"],
        &["--ask-for-approval", "on-request"],
        &["--permission-mode", "plan"],
        &[],
    ];
    for case in safe {
        let (verdict, flag, _) = screen(case);
        assert_eq!(verdict, Mind2tHostResult::Success, "{case:?} was refused, and it is not a bypass");
        assert!(flag.is_empty(), "{case:?} passed and still named a flag: {flag:?}");
    }
}

/// WHICH word to remove, not merely that there was one. A message naming the wrong flag sends
/// the operator hunting through an argv they already believe is fine.
#[test]
fn a_refusal_names_the_flag_and_where_it_sat() {
    let (verdict, flag, at) = screen(&["--verbose", "-a", "never"]);
    assert_eq!(verdict, Mind2tHostResult::Refused);
    assert_eq!(flag, "-a never");
    assert_eq!(at, 1);
}

/// A fake agent binary on a `PATH` of our own, made once for the whole test binary.
///
/// It answers the four things `launch::dress` is responsible for, in one line: where the child
/// landed, whether the Claude Code session marker was scrubbed, what TERM it was handed, and
/// the operator's own argv. Then `exec cat` so it stays alive - a child that exits is a
/// different verdict entirely, and this test is not about that.
fn fake_agent_path() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("mind2t-agent-abi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("fake agent dir");
        // `opencode` is a real registry id whose entry has no default args and no auto-approve
        // flag, so what reaches the child is exactly what the test passed. Shadowing rather
        // than requiring: our directory goes FIRST on PATH, so this works identically on a
        // machine that has the real one installed.
        let binary = dir.join("opencode");
        std::fs::write(
            &binary,
            "#!/bin/sh\nprintf 'AGENTGATE[%s][%s][%s][%s]\\n' \
             \"$PWD\" \"${CLAUDECODE:-scrubbed}\" \"$TERM\" \"$*\"\nexec cat\n",
        )
        .expect("write fake agent");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake agent");
        dir
    })
}

/// A quiet HOME the child can be asserted to have landed in.
fn fake_home() -> &'static PathBuf {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let home = std::env::temp_dir().join(format!("mind2t-agent-home-{}", std::process::id()));
        std::fs::create_dir_all(&home).expect("fake home");
        // Canonicalised because macOS hands out `/var/...` while the real path is
        // `/private/var/...`, and the child's own `$PWD` prints the resolved one.
        home.canonicalize().expect("canonical home")
    })
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

/// Every row of the last polled frame, joined without the row breaks - a pty WRAPS, and a
/// substring search against the row-broken form finds neither half of a wrapped marker.
fn screen_text(host: *mut Mind2tHost) -> String {
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
        }
    }
    out
}

/// Waits for the fake agent's marker line and returns it, or prints the whole screen and dies.
fn marker(host: *mut Mind2tHost) -> String {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        let seen = screen_text(host);
        if let Some(start) = seen.find("AGENTGATE[") {
            let tail = &seen[start..];
            // Four opened fields before the last bracket closes, so a frame caught mid-draw
            // is not read as a whole line. The child prints once and then `exec cat`s, so
            // nothing follows it on screen to confuse the search.
            if let Some(end) = tail.rfind(']')
                && tail[..=end].matches('[').count() >= 4
            {
                return tail[..=end].to_string();
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "the fake agent never reported. The screen held:\n{}\n(end of screen)",
        screen_text(host)
    );
}

/// Spawns a registry agent through the C surface, with our fake first on `PATH`.
///
/// Returns the raw result and the handle, so the refusal case can assert on both.
fn spawn_agent(id: &str, extra: &[&str]) -> (Mind2tHostResult, *mut Mind2tHost) {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("PATH").unwrap_or_default();
    let ours = std::env::join_paths(
        std::iter::once(fake_agent_path().clone())
            .chain(std::env::split_paths(&previous).collect::<Vec<_>>()),
    )
    .expect("joinable PATH");
    let parent_term = std::env::var_os("TERM");
    unsafe {
        std::env::set_var("PATH", &ours);
        std::env::set_var("HOME", fake_home());
        // A SENTINEL, not a removal, and this line is the difference between a control and a
        // decoration. The suite is itself run from a terminal, so the parent already has
        // TERM=xterm-256color; asserting the child has it would then pass whether `dress`
        // declared it or NOT. Measured on this test's first mutation run, where exactly that
        // happened: the cwd and session-marker assertions caught the missing `dress` and the
        // TERM one sat there green. With a sentinel, inheritance is the failing answer.
        std::env::set_var("TERM", "mind2t-inherited-not-declared");
    }

    let id = CString::new(id).expect("id");
    let owned = argv(extra);
    let pointers: Vec<*const std::ffi::c_char> = owned.iter().map(|word| word.as_ptr()).collect();
    let options = Mind2tHostOptions {
        cols: COLS,
        rows: ROWS,
        font_size: 0.0,
        // NULL, and the point is that it is ignored: the child is the agent, not this.
        command: ptr::null(),
        auto_direction: false,
        config: ptr::null(),
        cwd: ptr::null(),
    };
    let mut host: *mut Mind2tHost = ptr::null_mut();
    let result = unsafe {
        mind2t_host_spawn_agent(
            &options,
            id.as_ptr(),
            if pointers.is_empty() {
                ptr::null()
            } else {
                pointers.as_ptr()
            },
            pointers.len(),
            &mut host,
        )
    };
    unsafe {
        std::env::set_var("PATH", previous);
        match parent_term {
            Some(term) => std::env::set_var("TERM", term),
            None => std::env::remove_var("TERM"),
        }
    }
    (result, host)
}

/// THE GATE. An agent pane really runs the resolved binary, and the child gets the
/// environment `launch::dress` promises.
///
/// The four assertions are four separate live bugs this project has already paid for: a pane
/// opening on the sealed read-only root, an agent quietly turning its own transcript saving
/// off because it inherited a session marker, a Finder-launched child finding no terminfo and
/// exiting before drawing a cell, and the operator's own flags never reaching the child.
#[test]
fn an_agent_pane_runs_the_resolved_binary_with_the_child_environment() {
    let (result, host) = spawn_agent("opencode", &["--model", "sonnet"]);
    assert_eq!(result, Mind2tHostResult::Success, "the agent pane did not spawn");
    assert!(!host.is_null());

    let line = marker(host);
    unsafe { mind2t_host_free(host) };

    let home = fake_home().to_str().expect("utf-8 home");
    assert!(
        line.contains(&format!("AGENTGATE[{home}]")),
        "the agent did not land in HOME: {line}"
    );
    assert!(
        line.contains("[scrubbed]"),
        "CLAUDE Code's session marker survived into the agent: {line}"
    );
    assert!(
        line.contains("[xterm-256color]"),
        "the agent was handed no usable TERM: {line}"
    );
    assert!(
        line.contains("[--model sonnet]"),
        "the operator's own flags never reached the agent: {line}"
    );
}

/// THE CONTROL, and it is what makes the guard mean anything at this seam.
///
/// A bypass must be REFUSED rather than stripped: stripping it leaves the operator believing
/// approvals are off while the agent asks on every edit, which is the worse of the two
/// failures. And the refusal must be about the FLAG, not about the spawn path being broken -
/// so the near-miss half above spawns for real through the same call.
#[test]
fn a_bypass_is_refused_and_never_reaches_the_child() {
    let (refused, host) = spawn_agent("opencode", &["--yolo"]);
    assert_eq!(refused, Mind2tHostResult::Refused);
    assert!(host.is_null(), "a refused launch still produced a handle");

    // The same shape without the bypass. Without this the test above would pass on a
    // spawn_agent that refused everything it was ever handed.
    let (allowed, host) = spawn_agent("opencode", &["--autosave"]);
    assert_eq!(
        allowed,
        Mind2tHostResult::Success,
        "a near-miss flag was refused, and it is not a bypass"
    );
    let line = marker(host);
    unsafe { mind2t_host_free(host) };
    assert!(line.contains("[--autosave]"), "the near-miss flag was dropped: {line}");
}

/// An id nobody has, an id that is not in the registry, and an id that resolves - three
/// answers the embedder has to be able to tell apart, because only the middle one is a bug in
/// the caller and only the first has a useful thing to show the operator.
#[test]
fn resolve_tells_unknown_apart_from_missing_apart_from_installed() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    let nonsense = CString::new("not-an-agent").expect("id");
    let mut len = 0usize;
    assert_eq!(
        unsafe { mind2t_agent_resolve(nonsense.as_ptr(), ptr::null_mut(), 0, &mut len) },
        Mind2tHostResult::InvalidValue,
        "an id outside the registry was answered instead of refused"
    );

    // A registry id with nothing on PATH to find. `droid` rather than `opencode`: the probe
    // caches a hit for five minutes, so reusing the shadowed id here would answer from the
    // launch test's cache and this assertion would be about nothing.
    let droid = CString::new("droid").expect("id");
    let previous = std::env::var_os("PATH").unwrap_or_default();
    unsafe { std::env::set_var("PATH", "") };
    let missing = unsafe { mind2t_agent_resolve(droid.as_ptr(), ptr::null_mut(), 0, &mut len) };
    unsafe { std::env::set_var("PATH", &previous) };
    assert_eq!(
        missing,
        Mind2tHostResult::Ignored,
        "an agent that is not installed was reported as an error rather than as absent"
    );
    assert_eq!(len, 0, "a missing agent still reported a path length");

    // And one that does resolve, through the same call, so `Ignored` above is a verdict about
    // the machine rather than this function's only answer.
    let ours = std::env::join_paths(
        std::iter::once(fake_agent_path().clone())
            .chain(std::env::split_paths(&previous).collect::<Vec<_>>()),
    )
    .expect("joinable PATH");
    unsafe { std::env::set_var("PATH", &ours) };
    let opencode = CString::new("opencode").expect("id");
    let sized = unsafe { mind2t_agent_resolve(opencode.as_ptr(), ptr::null_mut(), 0, &mut len) };
    let mut buffer = vec![0u8; len];
    let filled =
        unsafe { mind2t_agent_resolve(opencode.as_ptr(), buffer.as_mut_ptr(), len, &mut len) };
    unsafe { std::env::set_var("PATH", previous) };

    assert_eq!(sized, Mind2tHostResult::Success);
    assert_eq!(filled, Mind2tHostResult::Success);
    let path = String::from_utf8(buffer).expect("utf-8 path");
    assert_eq!(
        path,
        fake_agent_path().join("opencode").to_str().expect("utf-8"),
        "the probe resolved a different binary than the one first on PATH"
    );
}
