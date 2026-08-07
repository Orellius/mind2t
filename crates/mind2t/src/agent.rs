//! Purpose: which coding-agent CLIs exist, how each one is launched, and what must never be
//!   typed on the operator's behalf.
//! Public surface: `AgentDef`, `REGISTRY`, `find`, `Probe`, `Dangerous`, `screen`, `launch`.
//! Why this file: this is the compatibility matrix for driving other people's agent CLIs, and
//!   every field in it is a behavioural difference that breaks a launch if guessed. It is DATA,
//!   recovered from a shipped product rather than invented, so it lives in one place with its
//!   provenance attached and nothing else in the host hardcodes a binary name or a flag.
//! NOT responsible for: spawning, panes, or reading what the agent then does. The registry says
//!   what to run; `Canvas` runs it and the typed grid says what happened (B4.2).
//! Test strategy: the guard is the part with teeth, so it is tested in BOTH directions - it must
//!   refuse the real dangerous flags AND stay quiet for the near-misses that merely look like
//!   them. A guard proven only on what it catches is the SCAR-004 shape: you cannot tell it from
//!   one that refuses everything, or from one that never fires.
//!
//! Provenance: the matrix is `ct` in BridgeSpace 3.4.17's MainApp chunk, carved 2026-08-05 and
//! written up in a teardown of the competing tools, kept outside this repository.
//! It is a compatibility fact about other people's software, not their code, and it is re-checked
//! against reality by `Probe` rather than trusted (SCAR-003).

use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, Instant};

/// How an agent takes its first prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompt {
    /// The prompt is an argv, per the agent's own template.
    Inline(&'static str),
    /// The agent must be launched bare and typed into afterwards. These need the alternate-screen
    /// settle detection and shorter idle windows - a prompt handed to them as argv is either
    /// ignored or taken as a file name.
    TypeAfterLaunch,
}

/// How a previous conversation is picked up again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// The agent hands out its own session id, which we store and pass back.
    NativeId { template: &'static str },
    /// No id, just "carry on with the last one".
    Continue { template: &'static str },
    None,
}

/// One agent CLI, as it must actually be driven.
#[derive(Debug, Clone)]
pub struct AgentDef {
    pub id: &'static str,
    pub name: &'static str,
    /// Probed IN ORDER. Several agents ship under more than one binary name and the first hit
    /// wins - `antigravity` answers to `agy` on a fresh install and to `gemini` on an older one.
    pub candidates: &'static [&'static str],
    pub default_args: &'static [&'static str],
    pub prompt: Prompt,
    /// The flag that turns off this agent's approvals. Recorded so it can be RECOGNISED and
    /// refused; see `screen`. It is never appended by us.
    pub auto_approve: Option<&'static str>,
    /// How long a launch may sit silent before it counts as failed rather than starting.
    pub spawn_grace: Duration,
    /// Silence after which a running agent is idle, when it differs from the default.
    pub idle: Option<Duration>,
    pub resume: Resume,
    pub install_hint: &'static str,
}

/// The matrix. Ten entries, of which this machine has four - the rest are here because the cost
/// of carrying data recovered once is zero and the cost of re-deriving it is a teardown.
pub const REGISTRY: &[AgentDef] = &[
    AgentDef {
        id: "claude",
        name: "Claude Code",
        candidates: &["claude"],
        default_args: &[],
        prompt: Prompt::Inline("{command} {prompt}"),
        auto_approve: Some("--dangerously-skip-permissions"),
        spawn_grace: Duration::from_secs(20),
        idle: None,
        resume: Resume::NativeId { template: "{command} --resume {id}" },
        install_hint: "npm i -g @anthropic-ai/claude-code",
    },
    AgentDef {
        id: "codex",
        name: "Codex",
        candidates: &["codex"],
        default_args: &[],
        prompt: Prompt::Inline("{command} {prompt}"),
        auto_approve: Some("-a never"),
        spawn_grace: Duration::from_secs(15),
        idle: None,
        resume: Resume::NativeId { template: "{command} resume {id}" },
        install_hint: "npm i -g @openai/codex",
    },
    AgentDef {
        id: "antigravity",
        name: "Antigravity",
        candidates: &["agy", "antigravity"],
        default_args: &[],
        prompt: Prompt::Inline("{command} -i {prompt}"),
        auto_approve: Some("--yolo"),
        spawn_grace: Duration::from_secs(15),
        idle: None,
        resume: Resume::None,
        install_hint: "curl -fsSL https://antigravity.google/cli/install.sh | bash",
    },
    AgentDef {
        id: "opencode",
        name: "opencode",
        candidates: &["opencode"],
        default_args: &[],
        prompt: Prompt::Inline("{command} --prompt {prompt}"),
        auto_approve: None,
        spawn_grace: Duration::from_secs(10),
        idle: None,
        resume: Resume::NativeId { template: "{command} --session {id}" },
        install_hint: "npm i -g opencode",
    },
    AgentDef {
        id: "gemini",
        name: "Gemini CLI",
        candidates: &["gemini"],
        default_args: &[],
        prompt: Prompt::Inline("{command} -i {prompt}"),
        auto_approve: Some("--yolo"),
        spawn_grace: Duration::from_secs(15),
        idle: None,
        resume: Resume::None,
        install_hint: "npm i -g @google/gemini-cli",
    },
    AgentDef {
        id: "cursor",
        name: "Cursor Agent",
        // The recovered matrix probes bare `agent` FIRST. Measured 2026-08-05 on this machine,
        // `agent` is `~/.grok/bin/agent` - Grok's binary - so that entry resolves "launch cursor"
        // to a different vendor's agent, silently and with a straight face. A generic candidate
        // name is not a candidate. Cost of dropping it: a machine with only `agent` installed
        // reports cursor missing, which is a false negative the operator can see and fix; the
        // alternative is a false POSITIVE that puts the wrong agent in their pane.
        candidates: &["cursor-agent"],
        default_args: &[],
        prompt: Prompt::Inline("{command} chat {prompt}"),
        auto_approve: Some("--yolo"),
        spawn_grace: Duration::from_secs(15),
        idle: None,
        resume: Resume::NativeId { template: "{command} resume {id}" },
        install_hint: "curl https://cursor.com/install -fsS | bash",
    },
    AgentDef {
        id: "copilot",
        name: "GitHub Copilot",
        candidates: &["copilot", "github-copilot"],
        default_args: &[],
        prompt: Prompt::Inline("{command} -p {prompt}"),
        auto_approve: Some("--allow-tool"),
        spawn_grace: Duration::from_secs(15),
        idle: None,
        resume: Resume::NativeId { template: "{command} --resume {id}" },
        install_hint: "npm i -g @github/copilot",
    },
    AgentDef {
        id: "droid",
        name: "Factory Droid",
        candidates: &["droid"],
        default_args: &[],
        prompt: Prompt::Inline("{command} exec {prompt}"),
        auto_approve: Some("--auto"),
        spawn_grace: Duration::from_secs(15),
        idle: None,
        resume: Resume::None,
        install_hint: "curl -fsSL https://app.factory.ai/cli | sh",
    },
    AgentDef {
        id: "grok",
        name: "Grok CLI",
        candidates: &["grok", "grok-build"],
        default_args: &[],
        prompt: Prompt::TypeAfterLaunch,
        auto_approve: Some("--always-approve"),
        spawn_grace: Duration::from_secs(20),
        idle: Some(Duration::from_secs(8)),
        resume: Resume::None,
        install_hint: "curl -fsSL https://x.ai/cli/install.sh | bash",
    },
    AgentDef {
        id: "kimi",
        name: "Kimi Code",
        candidates: &["kimi"],
        default_args: &[],
        prompt: Prompt::TypeAfterLaunch,
        auto_approve: Some("--yolo"),
        spawn_grace: Duration::from_secs(15),
        idle: Some(Duration::from_secs(8)),
        resume: Resume::Continue { template: "{command} --continue" },
        install_hint: "curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash",
    },
];

pub fn find(id: &str) -> Option<&'static AgentDef> {
    REGISTRY.iter().find(|agent| agent.id == id)
}

// -- the guard ---------------------------------------------------------------------------------

/// Every flag that turns an agent's approvals off, as WHOLE argv tokens.
///
/// Whole tokens, never substrings, and that distinction is the guard: `--auto` is Factory's
/// auto-approve and `--autosave` is not, so a `contains` here would refuse a harmless flag and
/// teach the operator to route around the guard. The two-word forms are matched as sequences for
/// the same reason - `never` on its own is a value, not a decision.
const DANGEROUS: &[&[&str]] = &[
    &["--yolo"],
    &["--dangerously-skip-permissions"],
    &["--dangerously-bypass-approvals-and-sandbox"],
    &["--full-auto"],
    &["--always-approve"],
    &["--auto"],
    &["--allow-tool"],
    &["-a", "never"],
    &["--ask-for-approval", "never"],
];

/// A refusal: which sequence was found, and where it sat in the argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dangerous {
    pub flag: String,
    pub at: usize,
}

/// Refuses to AUTO-TYPE an approval-bypassing flag on the operator's behalf.
///
/// The posture, copied deliberately from the product this registry came from: auto-approve is the
/// operator's decision to type, never ours to add. What makes it a real guard rather than a
/// comment is the second direction - it must stay silent for `--autosave`, `--model never-mind`
/// and every other near-miss, or it is indistinguishable from a function that refuses everything.
///
/// Returns the first offending sequence, or `None` when the argv is ours to send.
pub fn screen(argv: &[String]) -> Option<Dangerous> {
    for index in 0..argv.len() {
        for sequence in DANGEROUS {
            if argv.len() - index >= sequence.len()
                && argv[index..index + sequence.len()]
                    .iter()
                    .zip(*sequence)
                    .all(|(given, expected)| given == expected)
            {
                return Some(Dangerous {
                    flag: sequence.join(" "),
                    at: index,
                });
            }
        }
    }
    None
}

// -- availability ------------------------------------------------------------------------------

/// Which binary an agent resolves to on THIS machine, cached.
///
/// The registry is a claim about the world and the world disagrees with it constantly - agents
/// are installed, uninstalled and renamed under us. So availability is measured, and the cache
/// is asymmetric on purpose: a hit is stable for five minutes, a miss is re-checked after ten
/// seconds, because the interesting transition is an operator installing something mid-session
/// and wanting it to appear.
pub struct Probe {
    found: HashMap<&'static str, (Option<String>, Instant)>,
    hit: Duration,
    miss: Duration,
}

impl Default for Probe {
    fn default() -> Probe {
        Probe {
            found: HashMap::new(),
            hit: Duration::from_secs(300),
            miss: Duration::from_secs(10),
        }
    }
}

impl Probe {
    /// The absolute path of the first candidate that exists, or `None`.
    pub fn resolve(&mut self, agent: &'static AgentDef) -> Option<String> {
        if let Some((cached, at)) = self.found.get(agent.id) {
            let fresh = if cached.is_some() { self.hit } else { self.miss };
            if at.elapsed() < fresh {
                return cached.clone();
            }
        }
        let resolved = agent.candidates.iter().find_map(|candidate| which(candidate));
        self.found
            .insert(agent.id, (resolved.clone(), Instant::now()));
        resolved
    }

    /// Every agent the machine can actually run, in registry order.
    pub fn available(&mut self) -> Vec<(&'static AgentDef, String)> {
        REGISTRY
            .iter()
            .filter_map(|agent| self.resolve(agent).map(|path| (agent, path)))
            .collect()
    }
}

/// Where a binary lives, or `None`.
///
/// `PATH` is walked here rather than shelling out to `which`: the answer is needed before a pane
/// exists, spawning a process to ask whether we can spawn a process is a needless fork, and the
/// exec bit is the thing that actually decides it.
fn which(binary: &str) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;

    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(binary);
        let Ok(metadata) = std::fs::metadata(&candidate) else {
            continue;
        };
        // A directory named `claude` on the PATH is not a claude, and a file with no exec bit is
        // not runnable - both would otherwise resolve and fail at spawn, one layer too late.
        if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

// -- launching ---------------------------------------------------------------------------------

/// The command that starts `agent`, with nothing added that the operator did not ask for.
///
/// `extra` is the operator's own argv and is SCREENED, not sanitised: a dangerous flag comes back
/// as an error rather than being silently dropped, because quietly removing a flag someone typed
/// is worse than refusing it - they would believe approvals were off when they were not.
pub fn launch(
    agent: &'static AgentDef,
    binary: &str,
    extra: &[String],
) -> Result<Command, Dangerous> {
    if let Some(found) = screen(extra) {
        return Err(found);
    }
    let mut command = Command::new(binary);
    command.args(agent.default_args);
    command.args(extra);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::{Dangerous, Prompt, REGISTRY, find, launch, screen};

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    /// The direction everybody writes. On its own it cannot tell this guard from one that refuses
    /// every argv it is ever handed.
    #[test]
    fn the_guard_refuses_every_approval_bypass() {
        let bypasses: &[&[&str]] = &[
            &["--yolo"],
            &["--dangerously-skip-permissions"],
            &["--dangerously-bypass-approvals-and-sandbox"],
            &["--full-auto"],
            &["--always-approve"],
            &["--auto"],
            &["-a", "never"],
            &["--ask-for-approval", "never"],
            // Buried mid-argv, because a guard that only checks the first word is a guard that
            // anything gets past by putting one flag in front of it.
            &["--model", "sonnet", "--yolo", "--verbose"],
        ];
        for case in bypasses {
            assert!(
                screen(&argv(case)).is_some(),
                "{case:?} was allowed through the guard"
            );
        }
    }

    /// The direction that makes the first one mean something. Every one of these LOOKS like a
    /// bypass and is not, and a guard that refuses them is one the operator learns to work
    /// around - which is how a guard becomes worse than no guard at all.
    #[test]
    fn the_guard_stays_quiet_for_near_misses() {
        let safe: &[&[&str]] = &[
            &["--autosave"],
            &["--auto-detect"],
            &["--yolonaut"],
            &["--model", "never"],
            &["never"],
            &["-a"],
            &["--ask-for-approval", "on-request"],
            &["--permission-mode", "plan"],
            &[],
        ];
        for case in safe {
            assert_eq!(
                screen(&argv(case)),
                None,
                "{case:?} was refused, and it is not a bypass"
            );
        }
    }

    /// Where the offence is, not merely that there was one - the operator has to be told which
    /// word to remove, and a message naming the wrong one sends them hunting.
    #[test]
    fn a_refusal_names_the_flag_and_its_position() {
        let found = screen(&argv(&["--verbose", "-a", "never"])).expect("refused");
        assert_eq!(
            found,
            Dangerous {
                flag: "-a never".to_string(),
                at: 1
            }
        );
    }

    /// A refused launch produces NO command. The failure mode this pins is the tempting one:
    /// stripping the flag and launching anyway, which leaves the operator believing approvals
    /// are off while the agent asks for permission on every edit.
    #[test]
    fn a_dangerous_flag_refuses_the_launch_rather_than_being_dropped() {
        let claude = find("claude").expect("claude is in the registry");
        let refused = launch(claude, "/usr/bin/true", &argv(&["--yolo"]));
        assert!(refused.is_err(), "the launch was built with a bypass in it");

        let allowed = launch(claude, "/usr/bin/true", &argv(&["--permission-mode", "plan"]))
            .expect("a safe argv builds");
        let rendered = format!("{allowed:?}");
        assert!(
            rendered.contains("plan"),
            "the operator's own flags did not reach the command: {rendered}"
        );
    }

    /// No binary name may be claimed by two agents, and no agent may claim a name generic enough
    /// to belong to somebody else.
    ///
    /// This is not hygiene, it is the defect the probe found on its first run: the recovered
    /// matrix probes bare `agent` for cursor, and on this machine `agent` is Grok's binary, so
    /// "launch cursor" starts Grok. Nothing errors - the operator gets a working agent that is
    /// the wrong one. The same shape sat in the original table between antigravity and gemini.
    #[test]
    fn no_two_agents_can_resolve_to_the_same_binary() {
        let mut claimed: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        for agent in REGISTRY {
            for candidate in agent.candidates {
                if let Some(owner) = claimed.insert(candidate, agent.id) {
                    panic!(
                        "{candidate:?} is probed by both {owner} and {} - whichever is listed \
                         first wins and the other silently launches the wrong vendor's agent",
                        agent.id
                    );
                }
            }
        }

        // Names too generic to belong to anyone. Measured, not imagined: `agent` really is taken
        // on this machine, by Grok.
        for generic in ["agent", "ai", "cli", "code", "assistant"] {
            assert!(
                !claimed.contains_key(generic),
                "{generic:?} is a candidate binary; it is generic enough to be somebody else's"
            );
        }
    }

    /// The probe, against a DIFFERENT resolver - the shell's own `command -v`.
    ///
    /// Asserting that this machine has `claude` would pin the test to this machine; asserting
    /// that our PATH walk agrees with the system's makes it true anywhere and still catches the
    /// real defects: the exec-bit check, the directory-that-shares-a-binary's-name case, and
    /// candidate ORDER. Rust comparing its own arithmetic would pass on all three.
    #[test]
    fn the_probe_agrees_with_the_shell_about_what_is_installed() {
        // Held for the whole comparison: this test asks the SHELL what is on PATH, so it cannot
        // run while a sibling has PATH emptied. See the note on the cache test.
        let _guard = env_lock();

        let mut probe = super::Probe::default();
        let mut seen = 0;
        for agent in REGISTRY {
            let ours = probe.resolve(agent);
            // The shell is asked the same question in the same order, so a disagreement is about
            // resolution and not about which candidate was preferred.
            let theirs = agent.candidates.iter().find_map(|candidate| {
                // Asking the shell can itself fail under the suite's fork pressure, and an
                // `.ok()?` here read that failure as "not installed" - a None carrying two
                // meanings. Measured 2026-08-06: one full-suite run reported claude missing
                // while the same question, asked alone, finds it. A failed spawn is retried
                // and then loud; it is never a verdict about what is installed.
                let mut last_err = None;
                for attempt in 0..3u32 {
                    if attempt > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(25 << attempt));
                    }
                    match std::process::Command::new("/bin/sh")
                        .arg("-c")
                        .arg(format!("command -v {candidate}"))
                        .output()
                    {
                        Ok(output) => {
                            return output
                                .status
                                .success()
                                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
                                .filter(|path| !path.is_empty());
                        }
                        Err(err) => last_err = Some(err),
                    }
                }
                panic!("could not ask the shell about {candidate}: {last_err:?}");
            });
            assert_eq!(
                ours, theirs,
                "{}: the probe says {ours:?} and the shell says {theirs:?}",
                agent.id
            );
            if ours.is_some() {
                seen += 1;
            }
        }
        // Not an assertion about WHICH agents exist - just that the pair were compared against a
        // real installation somewhere, so a probe that always answered `None` cannot pass here on
        // the machine this project is developed on.
        println!("probe: {seen} of {} agents installed", REGISTRY.len());
    }

    /// A second `resolve` inside the cache window must not walk the PATH again.
    ///
    /// Proven by making the answer STALE and observing it survive: the cached value is kept and
    /// returned even though the world has changed. Without this, the cache could be absent
    /// entirely and every test above would still pass - it only ever affects how often we look.
    /// Serialises the tests that read or write the process environment.
    ///
    /// Poison is ignored deliberately: a panicking test has already failed and reported itself, and
    /// turning that into a second, unrelated failure in every test that follows only buries the
    /// first one.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn a_resolved_agent_is_cached_rather_than_re_walked() {
        // The environment is PROCESS-global and Rust runs tests in parallel threads, so the window
        // where PATH is empty below is a window in which every other test in this binary sees an
        // empty PATH too. Measured 2026-08-06: that raced
        // `the_probe_agrees_with_the_shell_about_what_is_installed`, whose shell then reported
        // claude missing while our own cached probe still had it - a red suite, green on re-run,
        // with nothing wrong in the product. Both tests take this lock; it is the reason
        // `set_var` is unsafe in edition 2024 and the reason a re-run must never be the fix.
        let _guard = env_lock();

        let mut probe = super::Probe::default();
        let claude = find("claude").expect("claude is in the registry");
        let first = probe.resolve(claude);

        // A PATH with nothing on it: a fresh walk MUST answer None, so anything else can only
        // have come from the cache.
        let saved = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", "") };
        let cached = probe.resolve(claude);
        match saved {
            Some(path) => unsafe { std::env::set_var("PATH", path) },
            None => unsafe { std::env::remove_var("PATH") },
        }

        assert_eq!(
            cached, first,
            "the answer changed inside the cache window, so the PATH was walked again"
        );
    }

    /// Every entry is reachable by the id the rest of the host will name it by, and both prompt
    /// strategies survive - if the type-after-launch agents ever vanish from this table, the code
    /// paths that exist only for them become dead without a compile error.
    #[test]
    fn the_registry_is_addressable_and_covers_both_prompt_strategies() {
        for agent in REGISTRY {
            assert!(
                find(agent.id).is_some(),
                "{} is in the table and cannot be found by id",
                agent.id
            );
            assert!(
                !agent.candidates.is_empty(),
                "{} has no binary to probe for",
                agent.id
            );
        }
        assert!(
            REGISTRY
                .iter()
                .any(|agent| agent.prompt == Prompt::TypeAfterLaunch),
            "no type-after-launch agent remains, so its handling is dead code"
        );
        assert!(
            REGISTRY
                .iter()
                .any(|agent| matches!(agent.prompt, Prompt::Inline(_))),
            "no inline-arg agent remains"
        );
    }
}
