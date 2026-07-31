//! Purpose: the command-history store and prefix matcher behind S4's fish-style
//!   ghost suggestions.
//! Public surface: `History`, `suggest`.
//! Why this file: the suggestion rule is the piece the backlog names as the oracle --
//!   "deterministic history fixture -> expected suggestion" -- so it lives in Rust
//!   where it is unit-tested, not improvised in the view layer. The GUI records a
//!   command when a block closes (the OSC 133 rails S2 laid) and asks for a
//!   suggestion against the input cells it already reads per tick.
//! NOT responsible for: knowing what the user typed (the GUI reads input-marked
//!   cells), drawing ghost text (the view), or accepting a suggestion (the view's
//!   right-arrow sends the remainder through paste).
//! Test strategy: deterministic fixtures below -- recency wins, a prefix must be
//!   PROPER (equal input suggests nothing), consecutive duplicates collapse, the cap
//!   drops oldest, and the file round-trips.

use std::path::Path;

/// One executed command, and where it was executed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    command: String,
    /// The normalized directory, when OSC 7 told us one. `None` is ordinary: a shell
    /// without the integration reports nothing, and its history still works globally.
    cwd: Option<String>,
}

/// Most-recent-last command history, keyed loosely by directory.
///
/// On disk one entry per line, `cwd\tcommand`; a line with no tab is a command with no
/// directory, which is exactly what the pre-cwd format produced, so old history files load
/// unchanged instead of being silently discarded.
#[derive(Debug, Default)]
pub struct History {
    /// Oldest first; the matcher walks it backward.
    entries: Vec<Entry>,
}

/// The cap the store prunes to. Fish keeps orders of magnitude more; ten thousand
/// commands is months of real use and keeps the prefix walk trivially fast.
const CAP: usize = 10_000;

impl History {
    pub fn load(path: &Path) -> History {
        let Ok(text) = std::fs::read_to_string(path) else {
            return History::default();
        };
        History {
            entries: text
                .lines()
                .filter(|line| !line.is_empty())
                .map(|line| match line.split_once('\t') {
                    Some((cwd, command)) => Entry {
                        command: command.to_string(),
                        cwd: (!cwd.is_empty()).then(|| cwd.to_string()),
                    },
                    // The pre-cwd format: a bare command line.
                    None => Entry { command: line.to_string(), cwd: None },
                })
                .collect(),
        }
    }

    /// Appends one executed command. Blank, multiline, and consecutive-duplicate
    /// commands are dropped -- a duplicate elsewhere in history is fine (recency
    /// matters), only immediate repeats add nothing.
    pub fn append(&mut self, command: &str, cwd: Option<&str>) {
        let command = command.trim();
        if command.is_empty()
            || command.contains('\n')
            || self
                .entries
                .last()
                .is_some_and(|last| last.command == command && last.cwd.as_deref() == cwd)
        {
            return;
        }
        self.entries.push(Entry {
            command: command.to_string(),
            cwd: cwd.map(str::to_string),
        });
        if self.entries.len() > CAP {
            let excess = self.entries.len() - CAP;
            self.entries.drain(..excess);
        }
    }

    /// Writes the whole store back. Atomic enough for a history file: a temp write
    /// plus rename, so a crash mid-save keeps the old file rather than half a one.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("tmp");
        let text: String = self
            .entries
            .iter()
            .map(|entry| format!("{}\t{}\n", entry.cwd.as_deref().unwrap_or(""), entry.command))
            .collect();
        std::fs::write(&temp, text)?;
        std::fs::rename(&temp, path)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The most recent entry `input` is a PROPER prefix of. Case-sensitive: history
    /// is literal shell text, and `Git` is not `git`. Empty input suggests nothing
    /// (a ghost on a bare prompt is noise, fish's own rule).
    /// A directory match is PREFERRED, not required: fish suggests what you ran here
    /// before anything else, and falls back to what you ran anywhere. Requiring the
    /// directory would make the ghost vanish the moment you `cd` somewhere new, which is
    /// worse than the behaviour this replaced.
    pub fn suggest(&self, input: &str, cwd: Option<&str>) -> Option<&str> {
        if input.is_empty() {
            return None;
        }
        let matches = |entry: &&Entry| {
            entry.command.len() > input.len() && entry.command.starts_with(input)
        };
        cwd.and_then(|here| {
            self.entries
                .iter()
                .rev()
                .filter(|entry| entry.cwd.as_deref() == Some(here))
                .find(matches)
        })
        .or_else(|| self.entries.iter().rev().find(matches))
        .map(|entry| entry.command.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(entries: &[&str]) -> History {
        let mut history = History::default();
        for entry in entries {
            history.append(entry, None);
        }
        history
    }

    #[test]
    fn the_most_recent_match_wins() {
        let history = history(&["git status", "git stash", "ls", "git st"]);
        // "git st" itself is the newest but equals-length entries never suggest;
        // "git stash" is more recent than "git status".
        assert_eq!(history.suggest("git s", None), Some("git st"));
        assert_eq!(history.suggest("git sta", None), Some("git stash"));
    }

    #[test]
    fn an_exact_match_suggests_nothing() {
        let history = history(&["ls -la"]);
        assert_eq!(history.suggest("ls -la", None), None, "nothing left to accept");
        assert_eq!(history.suggest("ls ", None), Some("ls -la"));
    }

    #[test]
    fn empty_input_and_case_mismatches_suggest_nothing() {
        let history = history(&["git status"]);
        assert_eq!(history.suggest("", None), None);
        assert_eq!(history.suggest("Git", None), None, "history is literal shell text");
    }

    #[test]
    fn consecutive_duplicates_collapse_but_recency_still_updates() {
        let mut history = history(&["ls", "ls", "ls"]);
        assert_eq!(history.len(), 1);
        history.append("git status", None);
        history.append("ls -l", None);
        history.append("ls", None);
        assert_eq!(history.len(), 4, "a later re-run is a new entry, not a dupe");
        // The newest match wins even when a longer one exists: "ls" re-ran last.
        assert_eq!(history.suggest("l", None), Some("ls"));
        assert_eq!(history.suggest("ls ", None), Some("ls -l"));
    }

    #[test]
    fn blank_and_multiline_commands_are_refused() {
        let mut history = History::default();
        history.append("   ", None);
        history.append("line one\nline two", None);
        assert!(history.is_empty());
    }

    #[test]
    fn the_cap_drops_oldest() {
        let mut history = History::default();
        for index in 0..(super::CAP + 10) {
            history.append(&format!("command-{index}"), None);
        }
        assert_eq!(history.len(), super::CAP);
        assert_eq!(history.suggest("command-0 ", None), None);
        assert!(history.suggest("command-999", None).is_some());
    }

    #[test]
    fn the_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("ruuah-hist-test-{}", std::process::id()));
        let path = dir.join("history");
        let mut history = History::default();
        history.append("git status", None);
        history.append("ls -la", None);
        history.save(&path).expect("save");
        let reloaded = History::load(&path);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(reloaded.len(), 2);
        assert_eq!(reloaded.suggest("git", None), Some("git status"));
    }

    /// The point of the slice: what you ran HERE outranks what you ran more recently
    /// somewhere else. Without the preference the newest entry always wins and the
    /// directory is decoration.
    #[test]
    fn a_match_in_this_directory_beats_a_newer_one_elsewhere() {
        let mut history = History::default();
        history.append("cargo test --workspace", Some("/work/ruuah"));
        history.append("cargo build --release", Some("/other"));

        assert_eq!(
            history.suggest("cargo ", Some("/work/ruuah")),
            Some("cargo test --workspace"),
            "the older entry wins because it was run here"
        );
        assert_eq!(
            history.suggest("cargo ", Some("/other")),
            Some("cargo build --release")
        );
    }

    /// A directory with nothing matching still suggests: falling back is what stops the
    /// ghost vanishing the moment you cd somewhere new.
    #[test]
    fn an_unknown_directory_falls_back_to_the_newest_match_anywhere() {
        let mut history = History::default();
        history.append("cargo test", Some("/work"));

        assert_eq!(history.suggest("cargo", Some("/somewhere/else")), Some("cargo test"));
        assert_eq!(history.suggest("cargo", None), Some("cargo test"));
    }

    /// The same command in two directories is not a consecutive duplicate: dropping it
    /// would leave the second directory with no local entry to prefer.
    #[test]
    fn the_same_command_in_a_different_directory_is_kept() {
        let mut history = History::default();
        history.append("ls -la", Some("/a"));
        history.append("ls -la", Some("/b"));
        history.append("ls -la", Some("/b"));

        assert_eq!(history.len(), 2, "repeated in ONE directory still collapses");
        assert_eq!(history.suggest("ls ", Some("/a")), Some("ls -la"));
    }

    /// History written before directories existed must keep working: a line with no tab
    /// is a command with no directory, which is exactly the old format.
    #[test]
    fn a_pre_cwd_history_file_loads_unchanged() {
        let dir = std::env::temp_dir().join(format!("ruuah-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("old-format");
        std::fs::write(&path, "git status\ncargo test\n").expect("write");

        let history = History::load(&path);
        assert_eq!(history.len(), 2);
        assert_eq!(history.suggest("cargo", None), Some("cargo test"));
        assert_eq!(
            history.suggest("cargo", Some("/anywhere")),
            Some("cargo test"),
            "entries with no directory are still reachable from one"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// And the round trip keeps the directory, or the preference would survive only
    /// until the session ended.
    #[test]
    fn the_directory_survives_a_save_and_load() {
        let dir = std::env::temp_dir().join(format!("ruuah-history-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("history");

        let mut history = History::default();
        history.append("make deploy", Some("/srv/app"));
        history.append("make check", Some("/other"));
        history.save(&path).expect("save");

        let reloaded = History::load(&path);
        assert_eq!(reloaded.suggest("make ", Some("/srv/app")), Some("make deploy"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
