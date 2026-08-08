//! Purpose: starting an agent CLI in a pane and KNOWING whether it started.
//! Public surface: `Verdict`, `Attempt`, `Launched`, `LaunchError`, `into_pane`, `observe`.
//! Why this file: this is the wedge, and it is one function call rather than a parser. Everyone
//!   else in this category watches an agent by scraping bytes out of a pty stream; we own the VT
//!   core, so "did it start" is a question asked of the TYPED GRID - `Session::visible_text` over
//!   the last drawn frame. No regex, no ANSI, no byte budget, and it cannot be confused by an
//!   agent that repaints its banner or clears the screen.
//! NOT responsible for: what the agent then DOES (B5 fuses grid semantics, CLI hooks and process
//!   polling into a state machine), stopping it (B4.3), or picking which agent goes where.
//! Test strategy: fakes for the machinery, one real agent for the claim. The state machine is
//!   proven with `/bin/sh` children whose behaviour is exactly specifiable - dies instantly,
//!   lives but says nothing, says something then dies, says something and stays - because those
//!   four are the whole truth table and a real CLI can only ever demonstrate one of them per run.
//!   The real-agent launch is a separate `#[ignore]`d test, run by hand: an automated suite must
//!   not start authenticated agent processes on someone's machine every time it runs.

use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

use mind2t_vt_host::session::{Session, SessionError};
use mind2t_vt_render::GpuContext;

use crate::agent::{AgentDef, Dangerous, launch};

/// What the pane's child turned out to be doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// It wrote something and was still alive afterwards. This is the only success.
    Running,
    /// Alive for the whole grace window and never wrote a byte. A wrong binary, a hung network
    /// call at startup, a CLI waiting on a tty it does not think it has.
    Silent,
    /// It exited during the grace window. `wrote` separates the two flavours, and they are
    /// different bugs: silence then exit is usually a missing binary or a failed auth, while
    /// output then exit is the CLI printing usage or an error and giving up.
    Exited { wrote: bool },
}

/// One attempt, kept so a retry loop can be SEEN to have run rather than assumed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attempt {
    pub index: u32,
    pub verdict: Verdict,
    /// How long this attempt was observed for.
    pub took: Duration,
    /// How long we waited BEFORE starting it. Zero for the first.
    pub backoff: Duration,
}

/// A started agent, with the history of getting there.
pub struct Launched {
    pub session: Session,
    pub attempts: Vec<Attempt>,
}

/// Shows the attempts and nothing else: a `Session` owns a pty, a child and a GPU surface, none
/// of which has a useful debug rendering and all of which would bury the one thing a failure
/// message needs - what happened on each try.
impl std::fmt::Debug for Launched {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Launched")
            .field("attempts", &self.attempts)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum LaunchError {
    /// The operator's argv carried an approval bypass. Refused, never stripped.
    Refused(Dangerous),
    /// Every attempt failed; the verdicts say how.
    Failed(Vec<Attempt>),
    Session(SessionError),
}

/// How long a child must stay alive after its first output before we believe it.
///
/// Without it, a CLI that prints usage and exits is `Running` for the microsecond between the
/// write and the exit - and which side of that race a poll lands on is luck. With it, "it wrote
/// and it is still there" is a claim about a span rather than an instant.
const SETTLE: Duration = Duration::from_millis(250);

/// How often the session is polled while waiting. The pump publishes on its own thread; this is
/// only how quickly we notice.
const TICK: Duration = Duration::from_millis(10);

/// Watches a freshly spawned session until it proves itself or the grace window ends.
///
/// The output test is `visible_text` - the drawn grid, not the byte stream. An agent that clears
/// the screen and repaints, or that switches to the alternate screen, still moves the grid; a
/// byte-counting version of this would also "see" output that was nothing but a cursor query.
pub fn observe(session: &mut Session, grace: Duration) -> Verdict {
    let deadline = Instant::now() + grace;
    let mut wrote_at: Option<Instant> = None;

    loop {
        session.poll();
        let wrote = !session.visible_text().trim().is_empty();
        if wrote && wrote_at.is_none() {
            wrote_at = Some(Instant::now());
        }
        // Death is checked BEFORE success, so a child that wrote and then quit is reported as
        // what it is rather than caught mid-race as Running.
        if session.exited() {
            return Verdict::Exited { wrote };
        }
        if wrote_at.is_some_and(|at| at.elapsed() >= SETTLE) {
            return Verdict::Running;
        }
        if Instant::now() >= deadline {
            return if wrote {
                // It wrote inside the window and is alive; the settle simply had not elapsed.
                Verdict::Running
            } else {
                Verdict::Silent
            };
        }
        sleep(TICK);
    }
}

/// Starts `agent` in a pane of `width` x `height` physical pixels, retrying on failure.
///
/// Backoff is exponential from `base` and the waits are RECORDED, because a retry loop nobody
/// can see is the SCAR-004 shape - it looks identical whether it ran three times or never ran at
/// all. Every attempt's verdict and its preceding wait come back with the session, and the tests
/// assert on them rather than on "it worked in the end".
pub fn into_pane(
    gpu: &GpuContext,
    agent: &'static AgentDef,
    binary: &str,
    extra: &[String],
    cwd: Option<&str>,
    size: (u32, u32),
    font_size: f32,
    tries: u32,
    base: Duration,
) -> Result<Launched, LaunchError> {
    let mut attempts = Vec::new();

    for index in 0..tries.max(1) {
        // Doubling from the base: 0, base, base*2, base*4. The first attempt never waits - a
        // backoff before the first try is latency charged for nothing.
        let backoff = if index == 0 {
            Duration::ZERO
        } else {
            base * 2u32.pow(index - 1)
        };
        if !backoff.is_zero() {
            sleep(backoff);
        }

        let mut command = launch(agent, binary, extra).map_err(LaunchError::Refused)?;
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        // Fitted, never spawned-then-resized: an agent that prints a banner or a table sized on
        // startup bakes the wrong width into scrollback, where no later resize can reach it.
        let mut session =
            Session::spawn_fitted_on(gpu, command, size.0, size.1, font_size, None)
                .map_err(LaunchError::Session)?;

        let started = Instant::now();
        let verdict = observe(&mut session, agent.spawn_grace);
        attempts.push(Attempt {
            index,
            verdict,
            took: started.elapsed(),
            backoff,
        });

        if verdict == Verdict::Running {
            return Ok(Launched { session, attempts });
        }
        // A failed attempt's child is reaped here rather than left to a destructor further up:
        // the next attempt spawns another one, and two agents holding the same worktree is a
        // problem that surfaces as file corruption rather than as an error (SCAR-016 discipline -
        // reach the cleanup path deliberately).
        session.shutdown();
    }

    Err(LaunchError::Failed(attempts))
}

/// What every child spawned into a pane is handed, in ONE place.
///
/// EXTRACTED 2026-08-08 because it was not in one place, and the consequence was live: a real
/// `claude` launched into a pane reported "Transcript saving is off - inherited
/// CLAUDE_CODE_CHILD_SESSION marker". The rule existed and was correct; it just lived inside
/// `main.rs`'s `shell_from`, and three other places built a `Command` without it - this module's
/// own `shell_command`, `agent::launch`, and the probe host. Exactly the shape that had already
/// cost this project the font-family bug: a rule applied on one path and silently absent on the
/// rest.
///
/// TERM IS DECLARED, NEVER PASSED THROUGH. An app launched from Finder inherits no environment,
/// so a child handed an empty TERM finds no terminfo entry and every ncurses program exits before
/// drawing a cell. That is what "no CLI can run in here" looks like from outside, and it is
/// invisible whenever the app is started from a terminal that already had one set.
///
/// A TERMINAL WINDOW IS A SESSION BOUNDARY. `CLAUDECODE` and `CLAUDE_CODE_CHILD_SESSION` are
/// removed so an agent CLI opened in a pane is its own session rather than a child of whatever
/// launched Mind2t. Leaving them set does not fail loudly - the agent starts, and quietly turns
/// off transcript saving.
pub fn dress(command: &mut Command) {
    command.env("TERM", CHILD_TERM);
    // Declared for the same reason as TERM rather than merely echoed: without it a program that
    // checks for truecolor falls back to 256 colours in a terminal that has had 24-bit colour
    // since slice 1.
    command.env("COLORTERM", "truecolor");
    command.env_remove("CLAUDECODE");
    command.env_remove("CLAUDE_CODE_CHILD_SESSION");
}

/// The terminal type every pane declares.
pub const CHILD_TERM: &str = "xterm-256color";

/// The command a pane would run for a plain shell, for callers mixing shells and agents.
///
/// `-l` for the same reason `shell_from` passes it: a shell with a pty on stdin is INTERACTIVE
/// already, so `.zshrc` was always sourced, but without `-l` neither `/etc/zprofile` (path_helper)
/// nor `~/.zprofile` (homebrew's shellenv) runs, and the pane's PATH is whatever launched the app.
pub fn shell_command() -> Command {
    let path = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let mut command = Command::new(path);
    command.arg("-l");
    dress(&mut command);
    command
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{LaunchError, Verdict, into_pane};
    use crate::agent::{AgentDef, Prompt, Resume};

    const SIZE: (u32, u32) = (640, 320);
    const FONT: f32 = 16.0;

    /// A fake agent whose "binary" is a shell script the test writes itself. The registry entry
    /// is real in shape - the same struct the host uses - so the machinery under test is the
    /// machinery that ships; only the child is specifiable.
    fn fake(grace_ms: u64) -> AgentDef {
        AgentDef {
            id: "fake",
            name: "Fake",
            candidates: &["sh"],
            default_args: &[],
            prompt: Prompt::Inline("{command} {prompt}"),
            auto_approve: None,
            spawn_grace: Duration::from_millis(grace_ms),
            idle: None,
            resume: Resume::None,
            install_hint: "-",
        }
    }

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    /// The whole truth table, in one test, because the four cases are only meaningful against
    /// each other: a verdict function that always said `Running` would pass any single one of
    /// them and none of the rest.
    #[test]
    fn every_shape_of_a_child_gets_its_own_verdict() {
        let agent: &'static AgentDef = Box::leak(Box::new(fake(600)));
        let gpu = mind2t_vt_render::GpuContext::new().expect("a GPU");

        let cases: &[(&str, Verdict)] = &[
            // Speaks and stays. The only success.
            ("printf 'hello from the agent\\n'; exec cat", Verdict::Running),
            // Alive and mute for the whole window: a hung startup, or a CLI waiting on something.
            // `exec sleep`, deliberately - the first version of this line was
            // `exec cat < /dev/null; sleep 30`, where the exec REPLACES the shell with a cat that
            // hits EOF at once, so the child died and the sleep never ran. It scored
            // `Exited { wrote: false }` and the fixture, not the code, was wrong.
            ("exec sleep 30", Verdict::Silent),
            // Dies instantly with nothing said: the missing-binary and failed-auth shape.
            ("exit 1", Verdict::Exited { wrote: false }),
            // Prints its usage and gives up. Reported as an exit, NOT as a success, which is the
            // case a naive "did anything appear on the grid" check gets wrong.
            ("printf 'usage: fake [options]\\n'; exit 2", Verdict::Exited { wrote: true }),
        ];

        for (script, want) in cases {
            let result = into_pane(
                &gpu,
                agent,
                "/bin/sh",
                &argv(&["-c", script]),
                None,
                SIZE,
                FONT,
                1,
                Duration::from_millis(0),
            );
            let got = match &result {
                Ok(launched) => launched.attempts.last().expect("an attempt").verdict,
                Err(LaunchError::Failed(attempts)) => {
                    attempts.last().expect("an attempt").verdict
                }
                Err(other) => panic!("{script:?} failed to even launch: {other:?}"),
            };
            assert_eq!(got, *want, "child {script:?}");
            if let Ok(mut launched) = result {
                launched.session.shutdown();
            }
        }
    }

    /// The retry loop, SEEN. Not "it eventually failed" - the attempts are counted, and the waits
    /// between them are required to have actually elapsed and to have doubled.
    ///
    /// This is the check the plan asks for by name, and it is the one that separates a retry loop
    /// from a comment claiming there is one: a loop that ran once and gave up produces exactly
    /// the same end result.
    #[test]
    fn a_failing_launch_retries_with_a_backoff_that_really_doubles() {
        let agent: &'static AgentDef = Box::leak(Box::new(fake(50)));
        let gpu = mind2t_vt_render::GpuContext::new().expect("a GPU");
        let base = Duration::from_millis(40);

        let started = std::time::Instant::now();
        let error = into_pane(
            &gpu,
            agent,
            "/bin/sh",
            &argv(&["-c", "exit 1"]),
            None,
            SIZE,
            FONT,
            3,
            base,
        )
        .expect_err("a child that exits immediately cannot be Running");

        let LaunchError::Failed(attempts) = error else {
            panic!("wrong error for a failing child");
        };
        assert_eq!(attempts.len(), 3, "the retry loop did not run three times");
        assert_eq!(
            attempts.iter().map(|attempt| attempt.backoff).collect::<Vec<_>>(),
            vec![Duration::ZERO, base, base * 2],
            "the backoff did not double"
        );
        // The waits were real, not merely recorded. Without this the struct could be filled in
        // with the numbers the assertion above wants while nothing ever slept.
        assert!(
            started.elapsed() >= base * 3,
            "the whole run took {:?}, which is less than the backoffs it claims to have waited",
            started.elapsed()
        );
    }

    /// A retry loop must not paper over a REFUSAL. An approval bypass is the operator's decision
    /// to make, so it fails once and immediately - retrying it three times would be three
    /// chances for a race to let it through, and it would look like flakiness rather than policy.
    #[test]
    fn a_refused_argv_never_reaches_a_second_attempt() {
        let agent: &'static AgentDef = Box::leak(Box::new(fake(50)));
        let gpu = mind2t_vt_render::GpuContext::new().expect("a GPU");

        let error = into_pane(
            &gpu,
            agent,
            "/bin/sh",
            &argv(&["--yolo"]),
            None,
            SIZE,
            FONT,
            3,
            Duration::from_millis(10),
        )
        .expect_err("a bypass must not launch");

        assert!(
            matches!(error, LaunchError::Refused(_)),
            "a bypass was retried instead of refused: {error:?}"
        );
    }

    /// A REAL agent CLI, in a real pane, verified from the typed grid.
    ///
    /// `#[ignore]`d deliberately: an automated suite must not start authenticated agent processes
    /// on the machine it runs on, every run, for as many agents as happen to be installed. This
    /// is run by hand when the launch path changes, and its result is recorded in the slice notes
    /// rather than assumed. Run it with:
    ///
    /// ```sh
    /// cargo test -p mind2t --lib launch::tests::a_real_agent -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "starts a real, authenticated agent CLI; run by hand"]
    fn a_real_agent_starts_and_is_seen_from_the_grid() {
        let mut probe = crate::agent::Probe::default();
        let claude = crate::agent::find("claude").expect("claude is in the registry");
        let Some(binary) = probe.resolve(claude) else {
            panic!("claude is not installed on this machine");
        };
        let gpu = mind2t_vt_render::GpuContext::new().expect("a GPU");

        let mut launched = into_pane(
            &gpu,
            claude,
            &binary,
            &[],
            None,
            (1200, 700),
            FONT,
            3,
            Duration::from_millis(200),
        )
        .expect("the agent started");

        // The wedge, stated as an assertion: what the agent put on screen is READ, not scraped.
        let screen = launched.session.visible_text();
        println!(
            "attempts: {:?}\n--- grid ---\n{}\n--- end ---",
            launched.attempts, screen
        );
        assert_eq!(launched.attempts.last().expect("an attempt").verdict, Verdict::Running);
        assert!(
            !screen.trim().is_empty(),
            "the agent is running and the grid is empty, so the sensor is reading nothing"
        );
        launched.session.shutdown();
    }
}
