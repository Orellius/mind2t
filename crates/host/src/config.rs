//! Purpose: `~/.ruuah/config.toml` and its theme files, parsed into one resolved value.
//! Public surface: `Config` and `Config::load`.
//! Why this file: the settings live behind the C surface (S1) so they are typed, unit-tested,
//!   and identical for every embedder -- a Swift-side parser would be a second implementation
//!   with no harness. The one rule: **a broken file never breaks the terminal.** Every parse
//!   failure falls back to the defaults AND surfaces as `Config::error`, which the GUI shows
//!   loudly. A theme that silently half-applies would be the SCAR-004 shape (looks like
//!   success, is a no-op), so errors accumulate and nothing partial is kept quiet.
//! NOT responsible for: applying the palette (lib.rs, at spawn/resize) or scalar precedence
//!   (the embedder owns CLI flags and Retina scaling, so it reads values and decides).
//! Test strategy: unit tests below cover both directions -- good files must resolve, and
//!   each malformed shape must produce BOTH the default fallback and a named error.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use ruuah_vt_render::Palette;
use serde::Deserialize;

/// The resolved settings: defaults, overridden by whatever parsed cleanly.
#[derive(Debug, Clone)]
pub struct Config {
    /// Font size in logical pixels. 0 means unset -- the embedder applies its default
    /// (and its backing-scale factor, which is why this is not resolved here).
    pub font_size: f32,
    /// `None` means unset -- the embedder keeps its own default (the .app is Hebrew-first).
    pub auto_direction: Option<bool>,
    /// Command line for new sessions, run via `/bin/sh -c`. `None` means the login $SHELL.
    pub shell: Option<String>,
    /// Lead font: an absolute path to a font file, or a name matched against installed
    /// font file stems (spaces/dashes ignored -- no CoreText name lookup, documented
    /// boundary). `None` keeps the built-in Menlo-led stack. A name that resolves to
    /// nothing is a loud config error and the stack stays default.
    pub font_family: Option<String>,
    /// Whether ASCII segments may form ligatures (needs a font that ships them; the
    /// default stack's Menlo has none, so this changes nothing until font-family does).
    pub font_ligatures: bool,
    /// The theme palette, resolved onto the built-in scheme. Always usable.
    pub palette: Palette,
    /// Every problem hit while loading, newline-joined. `None` when the load was clean.
    /// A missing config file is NOT an error (defaults are a valid state); a file that
    /// exists and cannot be honoured is.
    pub error: Option<String>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            font_size: 0.0,
            auto_direction: None,
            shell: None,
            font_family: None,
            font_ligatures: true,
            palette: Palette::default(),
            error: None,
        }
    }
}

/// `config.toml`, verbatim. Unknown keys are refused: a typo that silently does nothing
/// is worse than an error, and the error path here is a visible alert, not a crash.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(rename = "font-size")]
    font_size: Option<f32>,
    #[serde(rename = "auto-direction")]
    auto_direction: Option<bool>,
    shell: Option<String>,
    #[serde(rename = "font-family")]
    font_family: Option<String>,
    #[serde(rename = "font-ligatures")]
    font_ligatures: Option<bool>,
    theme: Option<String>,
}

/// `themes/<name>.toml`, verbatim. Every field optional: a theme that only sets the
/// background inherits everything else from the built-in scheme.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTheme {
    foreground: Option<String>,
    background: Option<String>,
    /// Exactly 16 entries when present -- the named system colours. The 6x6x6 cube and
    /// the grey ramp are not themeable; programs address them by absolute value.
    palette: Option<Vec<String>>,
}

impl Config {
    /// Loads `dir/config.toml` and, when it names one, `dir/themes/<name>.toml`.
    ///
    /// Never fails: the result is always a usable `Config`, and anything that could not
    /// be honoured is described in `error`. `dir` defaults to `~/.ruuah`.
    pub fn load(dir: Option<&Path>) -> Config {
        let dir = match dir {
            Some(dir) => dir.to_path_buf(),
            None => match std::env::var_os("HOME") {
                Some(home) => PathBuf::from(home).join(".ruuah"),
                None => return Config::default(),
            },
        };
        let mut config = Config::default();
        let mut errors = String::new();

        let path = dir.join("config.toml");
        let raw = match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<RawConfig>(&text) {
                Ok(raw) => raw,
                Err(error) => {
                    let _ = write!(errors, "{}: {}", path.display(), first_line(&error));
                    config.error = Some(errors);
                    return config;
                }
            },
            // Absent is the default state, not a problem.
            Err(_) => return config,
        };

        if let Some(size) = raw.font_size {
            if size.is_finite() && size > 0.0 && size <= 512.0 {
                config.font_size = size;
            } else {
                let _ = write!(errors, "font-size {size} is out of range (0, 512]");
            }
        }
        config.auto_direction = raw.auto_direction;
        config.shell = raw.shell.filter(|shell| !shell.is_empty());
        if let Some(family) = raw.font_family.filter(|family| !family.is_empty()) {
            if ruuah_vt_render::FontStack::family_resolves(&family) {
                config.font_family = Some(family);
            } else {
                if !errors.is_empty() {
                    errors.push('\n');
                }
                let _ = write!(errors, "font-family \"{family}\" matches no installed font");
            }
        }
        if let Some(on) = raw.font_ligatures {
            config.font_ligatures = on;
        }

        if let Some(name) = raw.theme {
            let path = dir.join("themes").join(format!("{name}.toml"));
            if let Err(error) = apply_theme(&path, &mut config.palette) {
                if !errors.is_empty() {
                    errors.push('\n');
                }
                let _ = write!(errors, "theme \"{name}\": {error}");
            }
        }

        config.error = (!errors.is_empty()).then_some(errors);
        config
    }
}

/// Reads one theme file onto the palette. Any error leaves the palette exactly as it
/// was -- the fallback is the whole built-in scheme, never a half-applied theme.
fn apply_theme(path: &Path, palette: &mut Palette) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let raw: RawTheme =
        toml::from_str(&text).map_err(|error| format!("{}: {}", path.display(), first_line(&error)))?;

    // Parse everything before touching the palette, so a bad entry cannot half-apply.
    let foreground = raw.foreground.as_deref().map(parse_hex).transpose()?;
    let background = raw.background.as_deref().map(parse_hex).transpose()?;
    let system = match raw.palette {
        Some(entries) => {
            if entries.len() != 16 {
                return Err(format!("palette has {} entries, needs exactly 16", entries.len()));
            }
            let mut colors = Vec::with_capacity(16);
            for entry in &entries {
                colors.push(parse_hex(entry)?);
            }
            Some(colors)
        }
        None => None,
    };

    if let Some(color) = foreground {
        palette.default_foreground = color;
    }
    if let Some(color) = background {
        palette.default_background = color;
    }
    if let Some(colors) = system {
        for (index, color) in colors.into_iter().enumerate() {
            palette.set_indexed(index as u8, color);
        }
    }
    Ok(())
}

/// `#rrggbb`, strictly. One format means an error message can name it exactly.
fn parse_hex(text: &str) -> Result<[u8; 4], String> {
    let hex = text
        .strip_prefix('#')
        .filter(|hex| hex.len() == 6)
        .ok_or_else(|| format!("\"{text}\" is not a #rrggbb color"))?;
    let value =
        u32::from_str_radix(hex, 16).map_err(|_| format!("\"{text}\" is not a #rrggbb color"))?;
    Ok([(value >> 16) as u8, (value >> 8) as u8, value as u8, 255])
}

/// toml's errors are multi-line with a caret diagram; an alert wants the sentence.
fn first_line(error: &impl std::fmt::Display) -> String {
    let text = error.to_string();
    text.lines().find(|line| !line.is_empty()).unwrap_or("parse error").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, relative: &str, text: &str) {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_missing_config_is_the_default_and_not_an_error() {
        let dir = tempdir();
        let config = Config::load(Some(&dir));
        assert_eq!(config.font_size, 0.0);
        assert!(config.error.is_none());
        assert_eq!(config.palette.default_background, Palette::default().default_background);
    }

    #[test]
    fn a_full_config_resolves_every_field() {
        let dir = tempdir();
        write(
            &dir,
            "config.toml",
            "font-size = 18.5\nauto-direction = false\nshell = \"/bin/bash\"\ntheme = \"night\"\n",
        );
        write(
            &dir,
            "themes/night.toml",
            "background = \"#204060\"\nforeground = \"#e0e0e0\"\n",
        );
        let config = Config::load(Some(&dir));
        assert_eq!(config.error, None);
        assert_eq!(config.font_size, 18.5);
        assert_eq!(config.auto_direction, Some(false));
        assert_eq!(config.shell.as_deref(), Some("/bin/bash"));
        assert_eq!(config.palette.default_background, [0x20, 0x40, 0x60, 255]);
        assert_eq!(config.palette.default_foreground, [0xe0, 0xe0, 0xe0, 255]);
    }

    #[test]
    fn malformed_toml_falls_back_to_defaults_and_says_so() {
        let dir = tempdir();
        write(&dir, "config.toml", "font-size = = 12\n");
        let config = Config::load(Some(&dir));
        assert_eq!(config.font_size, 0.0, "the default, not the broken value");
        let error = config.error.expect("a broken file must be loud");
        assert!(error.contains("config.toml"), "the error names the file: {error}");
    }

    #[test]
    fn an_unknown_key_is_refused_not_ignored() {
        // The typo trap: `fontsize = 20` silently doing nothing looks like a broken app.
        let dir = tempdir();
        write(&dir, "config.toml", "fontsize = 20\n");
        let config = Config::load(Some(&dir));
        assert!(config.error.is_some());
    }

    #[test]
    fn a_named_theme_that_does_not_exist_is_loud_and_keeps_the_scheme() {
        let dir = tempdir();
        write(&dir, "config.toml", "theme = \"ghost\"\n");
        let config = Config::load(Some(&dir));
        let error = config.error.expect("a missing named theme must be loud");
        assert!(error.contains("ghost"), "{error}");
        assert_eq!(config.palette.default_background, Palette::default().default_background);
    }

    #[test]
    fn a_bad_theme_entry_applies_nothing_at_all() {
        // background parses, palette does not -- a half-applied theme would show the new
        // background with the old colours and no error, which is exactly the silent shape
        // this module exists to refuse.
        let dir = tempdir();
        write(&dir, "config.toml", "theme = \"broken\"\n");
        write(
            &dir,
            "themes/broken.toml",
            "background = \"#204060\"\npalette = [\"nope\"]\n",
        );
        let config = Config::load(Some(&dir));
        assert!(config.error.is_some());
        assert_eq!(
            config.palette.default_background,
            Palette::default().default_background,
            "the background must NOT have been applied"
        );
    }

    #[test]
    fn a_sixteen_entry_palette_lands_on_the_named_colors_only() {
        let dir = tempdir();
        write(&dir, "config.toml", "theme = \"sixteen\"\n");
        let entries: Vec<String> = (0..16).map(|index| format!("\"#0000{index:02x}\"")).collect();
        write(
            &dir,
            "themes/sixteen.toml",
            &format!("palette = [{}]\n", entries.join(", ")),
        );
        let config = Config::load(Some(&dir));
        assert_eq!(config.error, None);
        assert_eq!(config.palette.indexed(0), [0, 0, 0x00, 255]);
        assert_eq!(config.palette.indexed(15), [0, 0, 0x0f, 255]);
        // The cube is not themeable; 196 stays xterm's bright red.
        assert_eq!(config.palette.indexed(196), [255, 0, 0, 255]);
    }

    #[test]
    fn font_size_out_of_range_is_loud_and_unset() {
        let dir = tempdir();
        write(&dir, "config.toml", "font-size = -3.0\n");
        let config = Config::load(Some(&dir));
        assert_eq!(config.font_size, 0.0);
        assert!(config.error.is_some());
    }

    #[test]
    fn hex_parsing_is_strict() {
        assert_eq!(parse_hex("#ff8000"), Ok([255, 128, 0, 255]));
        assert!(parse_hex("ff8000").is_err(), "the # is required");
        assert!(parse_hex("#ff800").is_err(), "six digits exactly");
        assert!(parse_hex("#ff80zz").is_err());
    }

    /// A per-test unique directory under the target tmp dir; leaked on purpose (the OS
    /// cleans tmp, and a Drop-deleting guard would hide files from a failing test's eyes).
    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ruuah-config-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn font_keys_parse_and_a_missing_family_is_loud() {
        let dir = std::env::temp_dir().join(format!("ruuah-config-fonts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "font-ligatures = false\nfont-family = \"NoSuchFontFamily9000\"\n",
        )
        .unwrap();
        let config = Config::load(Some(&dir));
        assert!(!config.font_ligatures);
        assert_eq!(config.font_family, None, "an unresolved family stays default");
        let error = config.error.expect("the miss is loud");
        assert!(error.contains("NoSuchFontFamily9000"), "{error}");

        std::fs::write(dir.join("config.toml"), "font-family = \"Menlo\"\n").unwrap();
        let config = Config::load(Some(&dir));
        assert_eq!(config.font_family.as_deref(), Some("Menlo"));
        assert!(config.font_ligatures, "default stays on");
        assert!(config.error.is_none(), "{:?}", config.error);
    }
}
