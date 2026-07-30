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

/// Most-recent-last command history. One command per line on disk; commands with
/// embedded newlines are refused at append (multiline history is a named v1
/// boundary, not a silent mangling).
#[derive(Debug, Default)]
pub struct History {
    /// Oldest first; the matcher walks it backward.
    entries: Vec<String>,
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
            entries: text.lines().map(str::to_string).filter(|line| !line.is_empty()).collect(),
        }
    }

    /// Appends one executed command. Blank, multiline, and consecutive-duplicate
    /// commands are dropped -- a duplicate elsewhere in history is fine (recency
    /// matters), only immediate repeats add nothing.
    pub fn append(&mut self, command: &str) {
        let command = command.trim();
        if command.is_empty()
            || command.contains('\n')
            || self.entries.last().is_some_and(|last| last == command)
        {
            return;
        }
        self.entries.push(command.to_string());
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
        std::fs::write(&temp, self.entries.join("\n") + "\n")?;
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
    pub fn suggest(&self, input: &str) -> Option<&str> {
        if input.is_empty() {
            return None;
        }
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.len() > input.len() && entry.starts_with(input))
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(entries: &[&str]) -> History {
        let mut history = History::default();
        for entry in entries {
            history.append(entry);
        }
        history
    }

    #[test]
    fn the_most_recent_match_wins() {
        let history = history(&["git status", "git stash", "ls", "git st"]);
        // "git st" itself is the newest but equals-length entries never suggest;
        // "git stash" is more recent than "git status".
        assert_eq!(history.suggest("git s"), Some("git st"));
        assert_eq!(history.suggest("git sta"), Some("git stash"));
    }

    #[test]
    fn an_exact_match_suggests_nothing() {
        let history = history(&["ls -la"]);
        assert_eq!(history.suggest("ls -la"), None, "nothing left to accept");
        assert_eq!(history.suggest("ls "), Some("ls -la"));
    }

    #[test]
    fn empty_input_and_case_mismatches_suggest_nothing() {
        let history = history(&["git status"]);
        assert_eq!(history.suggest(""), None);
        assert_eq!(history.suggest("Git"), None, "history is literal shell text");
    }

    #[test]
    fn consecutive_duplicates_collapse_but_recency_still_updates() {
        let mut history = history(&["ls", "ls", "ls"]);
        assert_eq!(history.len(), 1);
        history.append("git status");
        history.append("ls -l");
        history.append("ls");
        assert_eq!(history.len(), 4, "a later re-run is a new entry, not a dupe");
        // The newest match wins even when a longer one exists: "ls" re-ran last.
        assert_eq!(history.suggest("l"), Some("ls"));
        assert_eq!(history.suggest("ls "), Some("ls -l"));
    }

    #[test]
    fn blank_and_multiline_commands_are_refused() {
        let mut history = History::default();
        history.append("   ");
        history.append("line one\nline two");
        assert!(history.is_empty());
    }

    #[test]
    fn the_cap_drops_oldest() {
        let mut history = History::default();
        for index in 0..(super::CAP + 10) {
            history.append(&format!("command-{index}"));
        }
        assert_eq!(history.len(), super::CAP);
        assert_eq!(history.suggest("command-0 "), None);
        assert!(history.suggest("command-999").is_some());
    }

    #[test]
    fn the_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("ruuah-hist-test-{}", std::process::id()));
        let path = dir.join("history");
        let mut history = History::default();
        history.append("git status");
        history.append("ls -la");
        history.save(&path).expect("save");
        let reloaded = History::load(&path);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(reloaded.len(), 2);
        assert_eq!(reloaded.suggest("git"), Some("git status"));
    }
}
