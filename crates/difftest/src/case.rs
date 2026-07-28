//! Purpose: load the corpus of byte streams, each with the verdict it is expected to get.
//! Public surface: `Case`, `Expectation`, `load`, `CorpusError`, `DEFAULT_CORPUS`.
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
    #[serde(rename = "case", default)]
    cases: Vec<Case>,
}

/// Reads and parses a corpus file. An empty corpus is an error, not an empty pass.
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
    Ok(corpus.cases)
}
