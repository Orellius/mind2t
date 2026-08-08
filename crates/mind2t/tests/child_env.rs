//! Every spawn path hands its child the same environment.
//!
//! WHY THIS EXISTS, and it is a defect report rather than a precaution. On 2026-08-08 a real
//! `claude` launched into a pane reported:
//!
//!     Transcript saving is off - inherited CLAUDE_CODE_CHILD_SESSION marker
//!
//! The rule that prevents exactly that had existed since slice 8 and was correct. It just lived
//! inside `main.rs`'s `shell_from`, and three other places built a `Command` without it:
//! `launch::shell_command`, `agent::launch`, and the probe host. Nothing compared them, so the
//! divergence was invisible - the same shape that had already cost this project `font-family`
//! and `font-ligatures`, which were parsed and then applied on one path only.
//!
//! The fix is one function, `launch::dress`. This file is what keeps it one function: it asserts
//! the ENVIRONMENT MUTATIONS a built `Command` carries, without spawning anything. A test that
//! spawned would need a pty, a real shell and a login profile, and would then be measuring the
//! machine rather than the code.
//!
//! What this canNOT see, stated so the coverage is not overclaimed: whether the child actually
//! ends up with these values. Inheritance, profile scripts and the pty are all downstream of
//! here. `scripts/smoke-mind2t.sh` covers that end, by launching the host with a poisoned
//! environment and reading the values back off the grid.

use std::ffi::OsStr;
use std::process::Command;

/// The environment mutations a `Command` carries, as a sorted list of `(name, value)` where a
/// removal is `None`. `Command::get_envs` reports exactly the mutations, not the inherited
/// environment, which is what makes this assertable without running anything.
fn mutations(command: &Command) -> Vec<(String, Option<String>)> {
    let mut seen: Vec<(String, Option<String>)> = command
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    seen.sort();
    seen
}

/// What every child must be handed.
fn expected() -> Vec<(String, Option<String>)> {
    let mut want = vec![
        ("CLAUDECODE".to_string(), None),
        ("CLAUDE_CODE_CHILD_SESSION".to_string(), None),
        ("COLORTERM".to_string(), Some("truecolor".to_string())),
        (
            "TERM".to_string(),
            Some(mind2t::launch::CHILD_TERM.to_string()),
        ),
    ];
    want.sort();
    want
}

#[test]
fn dress_sets_the_terminal_type_and_removes_the_session_markers() {
    let mut command = Command::new("/bin/sh");
    mind2t::launch::dress(&mut command);
    assert_eq!(mutations(&command), expected());
}

/// The plain-shell path, which had NONE of this until today.
#[test]
fn the_shell_command_path_is_dressed() {
    let command = mind2t::launch::shell_command();
    assert_eq!(
        mutations(&command),
        expected(),
        "a shell spawned through launch::shell_command does not get the child environment"
    );
}

/// The agent path, which is the one the operator actually hit.
#[test]
fn the_agent_path_is_dressed() {
    let agent = mind2t::agent::REGISTRY
        .iter()
        .find(|agent| agent.id == "claude")
        .expect("the registry still lists claude");
    let command = mind2t::agent::launch(agent, "claude", &[]).expect("a plain launch is allowed");
    assert_eq!(
        mutations(&command),
        expected(),
        "an agent CLI spawned through agent::launch does not get the child environment - this is \
         the defect that turned off transcript saving in a real pane"
    );
}

/// The shell path also has to be a LOGIN shell.
///
/// Separate from the environment because it is a separate failure: without `-l`, neither
/// `/etc/zprofile` (path_helper) nor `~/.zprofile` (homebrew's shellenv) runs, so a pane opened
/// from Finder has whatever PATH the app was launched with. The symptom is "the tools are simply
/// absent", which reads as a broken terminal rather than a missing argument.
#[test]
fn the_shell_command_path_is_a_login_shell() {
    let command = mind2t::launch::shell_command();
    let args: Vec<&OsStr> = command.get_args().collect();
    assert!(
        args.iter().any(|arg| *arg == OsStr::new("-l")),
        "launch::shell_command is not a login shell; args were {args:?}"
    );
}

/// The control, and without it every assertion above passes against a `dress` that does nothing.
///
/// A bare `Command` must NOT satisfy the contract. If `expected()` were ever emptied, or
/// `mutations` returned nothing for any input, the four tests above would agree with each other
/// and with nothing real.
#[test]
fn an_undressed_command_fails_the_same_contract() {
    let bare = Command::new("/bin/sh");
    assert_ne!(
        mutations(&bare),
        expected(),
        "a command nobody dressed satisfies the contract; the comparison is measuring nothing"
    );
    assert!(mutations(&bare).is_empty());
}

/// THE OPERATOR'S ACTUAL PATH, reproduced: a shell pane, and `claude` TYPED into it.
///
/// Every other check in this file, and the live tap in `launch.rs`, exercises `agent::launch` -
/// the workbench's launcher. That is NOT how Orel starts an agent. He opens a pane, gets a
/// shell, and types the word. The shell is a second environment between the host and the agent,
/// and nothing in this repository had ever looked through it.
///
/// `#[ignore]`d for the same reason the other live tap is: a suite must not start authenticated
/// agent processes on somebody's machine. Run it deliberately:
///
///     cargo test -p mind2t --test child_env -- --ignored --nocapture
///
/// It sends no prompt, so nothing is spent. It prints the grid, which is the point: whatever
/// Claude Code says about its own session state is text on our own typed grid, read back with
/// no regex and no ANSI parsing.
#[test]
#[ignore = "starts a real, authenticated agent CLI; run by hand"]
fn claude_typed_into_a_shell_pane_reports_a_clean_session() {
    use std::time::{Duration, Instant};

    let gpu = mind2t_vt_render::GpuContext::new().expect("a GPU");
    let mut session = mind2t_vt_host::session::Session::spawn_fitted_on(
        &gpu,
        mind2t::launch::shell_command(),
        1200,
        700,
        16.0,
        None,
    )
    .expect("a shell pane");

    // Let the login shell finish its profile before typing, or the word lands in the middle of
    // whatever homebrew's shellenv is printing and the shell never sees a complete line.
    let settle = Instant::now();
    while settle.elapsed() < Duration::from_millis(1500) {
        session.poll();
        std::thread::sleep(Duration::from_millis(50));
    }

    session.send(b"claude\n").expect("the shell took the word");

    // Claude Code draws its banner and then its status line; the warning we are hunting sits
    // BELOW the input box, so this waits for the status line rather than for first output.
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(25) {
        session.poll();
        if session.visible_text().contains("Claude Code v") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // A further settle: the transcript warning appears after the model line, not with it.
    let extra = Instant::now();
    while extra.elapsed() < Duration::from_secs(4) {
        session.poll();
        std::thread::sleep(Duration::from_millis(100));
    }

    let screen = session.visible_text();
    println!("--- grid ---\n{screen}\n--- end ---");
    session.shutdown();

    assert!(
        screen.contains("Claude Code v"),
        "claude never drew its banner in the pane, so this test measured nothing"
    );
    assert!(
        !screen.contains("Transcript saving is off"),
        "claude reports transcript saving OFF when started from a shell pane"
    );
}
