//! Purpose: load the corpus of byte streams, each with the verdict it is expected to get.
//! Public surface: `Case`, `Resize`, `Expectation`, `load`, `CorpusError`, `DEFAULT_CORPUS`.
//! Why this file: a harness that is never wrong about anything is not evidence. Declaring
//!   the expected verdict per case makes the harness itself testable -- if the stub starts
//!   agreeing with Ghostty on SGR, that is a corpus failure demanding an explanation.
//! NOT responsible for: running anything (`run.rs`) or comparing anything (`ruuah-vt-snapshot`).
//! Test strategy: the corpus round-trip is covered by `tests/corpus.rs`, which loads the
//!   real file rather than a fixture.

use serde::Deserialize;

/// The corpus that ships with the repo.
pub const DEFAULT_CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/cases.toml");

#[derive(Debug, thiserror::Error)]
pub enum CorpusError {
    #[error("cannot read corpus at {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot parse corpus at {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },

    #[error("corpus at {path} declares no cases")]
    Empty { path: String },

    #[error(
        "corpus at {path} declares {declared} cases but loaded {found}; \
         if this was intentional, update `declared_cases`"
    )]
    CountMismatch {
        path: String,
        declared: usize,
        found: usize,
    },
}

/// What a case is expected to prove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Expectation {
    /// ruuah-vt already agrees with Ghostty here. A regression makes this case fail.
    Match,
    /// ruuah-vt does not implement this yet. When it does, this case fails and gets promoted.
    Diff,
}

impl std::fmt::Display for Expectation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Expectation::Match => write!(f, "match"),
            Expectation::Diff => write!(f, "diff"),
        }
    }
}

/// New dimensions to resize to, mid-case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Resize {
    pub cols: u16,
    pub rows: u16,
}

/// One byte stream, the geometry to run it at, and its expected verdict.
#[derive(Debug, Clone, Deserialize)]
pub struct Case {
    pub name: String,
    pub expect: Expectation,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
    /// Lines of scrollback to retain. Zero keeps the pre-slice-3 behaviour, so every case
    /// written before scrollback existed is unaffected.
    #[serde(default)]
    pub scrollback: usize,
    /// The stream to feed both implementations, as a TOML basic string. Control
    /// bytes use TOML's own escapes, so ESC is written u001B with a leading backslash.
    pub bytes: String,
    /// A resize applied after `bytes`. Absent means the case never resizes, which is every
    /// case written before slice 4.
    #[serde(default)]
    pub resize: Option<Resize>,
    /// A second stream, written after the resize. This is what makes the reflowed cursor
    /// position observable: where the next character lands is the only way to see it
    /// through a grid comparison.
    #[serde(default)]
    pub after: String,
    /// Observe what a renderer would have to repaint. The window is the `after` stream: the
    /// dirty flags are reset once `bytes` and any resize have been applied, so the damage
    /// reported is exactly what `after` changed.
    #[serde(default)]
    pub damage: bool,
    /// Why this case exists, in one line. Shown in the report.
    #[serde(default)]
    pub note: String,
}

fn default_cols() -> u16 {
    20
}

fn default_rows() -> u16 {
    5
}

#[derive(Debug, Deserialize)]
struct Corpus {
    /// How many cases this file is asserted to contain. Deliberately required rather than
    /// defaulted: a corpus that forgets to declare its size fails to parse, so the check
    /// cannot be dropped by omission.
    declared_cases: usize,
    #[serde(rename = "case", default)]
    cases: Vec<Case>,
}

/// Reads and parses a corpus file. An empty corpus is an error, not an empty pass.
///
/// The declared count is enforced here rather than in a test because every consumer goes
/// through this function: a corpus that silently loses cases takes the `difftest` binary down
/// with it instead of letting it report a confident verdict over a fraction of the corpus.
/// Mutation found this the hard way -- truncating the case list to 30 printed
/// "30/30 met expectation" and exited 0, with 67 cases gone and every gate green.
pub fn load(path: &str) -> Result<Vec<Case>, CorpusError> {
    let text = std::fs::read_to_string(path).map_err(|source| CorpusError::Read {
        path: path.to_string(),
        source,
    })?;
    let corpus: Corpus = toml::from_str(&text).map_err(|source| CorpusError::Parse {
        path: path.to_string(),
        source,
    })?;
    if corpus.cases.is_empty() {
        return Err(CorpusError::Empty {
            path: path.to_string(),
        });
    }
    if corpus.cases.len() != corpus.declared_cases {
        return Err(CorpusError::CountMismatch {
            path: path.to_string(),
            declared: corpus.declared_cases,
            found: corpus.cases.len(),
        });
    }
    Ok(corpus.cases)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a corpus with `declared` in the header and `actual` cases in the body.
    fn corpus_file(name: &str, declared: usize, actual: usize) -> String {
        let mut text = format!("declared_cases = {declared}\n");
        for i in 0..actual {
            text.push_str(&format!(
                "\n[[case]]\nname = \"c{i}\"\nexpect = \"match\"\nbytes = \"x\"\n"
            ));
        }
        let path = std::env::temp_dir().join(format!("ruuah-vt-{name}.toml"));
        std::fs::write(&path, text).expect("write the fixture");
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn a_corpus_whose_count_matches_its_header_loads() {
        let path = corpus_file("count-ok", 3, 3);
        assert_eq!(load(&path).expect("loads").len(), 3);
    }

    /// The control. Truncating the case list is exactly what a slicing bug does, and before
    /// this check the harness reported "30/30 met expectation" and exited 0 with 67 cases
    /// missing. A count that only ever grows would not catch it either -- the assertion has to
    /// be equality.
    #[test]
    fn a_corpus_that_lost_cases_is_refused() {
        let path = corpus_file("count-short", 97, 30);
        let error = load(&path).expect_err("a truncated corpus must not load");
        assert!(
            matches!(
                error,
                CorpusError::CountMismatch {
                    declared: 97,
                    found: 30,
                    ..
                }
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn a_corpus_that_gained_an_undeclared_case_is_refused() {
        let path = corpus_file("count-long", 2, 3);
        assert!(matches!(
            load(&path).expect_err("an undeclared case must not load"),
            CorpusError::CountMismatch { declared: 2, found: 3, .. }
        ));
    }

    /// Without this, a header-less corpus would parse and the floor would be silently absent
    /// in exactly the file it exists to protect.
    #[test]
    fn a_corpus_with_no_declared_count_does_not_parse() {
        let path = std::env::temp_dir().join("ruuah-vt-count-missing.toml");
        std::fs::write(
            &path,
            "[[case]]\nname = \"c\"\nexpect = \"match\"\nbytes = \"x\"\n",
        )
        .expect("write the fixture");

        assert!(matches!(
            load(&path.to_string_lossy()).expect_err("must not parse"),
            CorpusError::Parse { .. }
        ));
    }
}
