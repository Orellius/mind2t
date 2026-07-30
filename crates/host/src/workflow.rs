//! Purpose: `~/.ruuah/workflows/*.toml` -- parameterized command templates the cmd+K
//!   palette offers (S3, Warp's shape in our own plain-data format).
//! Public surface: `Workflow`, `WorkflowArg`, `load_dir`, `placeholders`, `render`.
//! Why this file: templates live behind the C surface for the same reason config does --
//!   typed, unit-tested, identical for every embedder. Substitution especially: the
//!   backlog names it as the piece that MUST be unit-tested, because a palette that
//!   renders `ssh {{host}}` with a placeholder left in it types garbage into a shell.
//! NOT responsible for: the palette UI (Swift), executing anything (the rendered string
//!   goes through the PASTE path, so the user still presses Enter themselves), or
//!   config.toml (config.rs).
//! Test strategy: unit tests cover both directions -- files that parse must resolve
//!   with their placeholders discovered in order, and each failure shape (broken TOML,
//!   missing fields, unresolved placeholder at render) must be a named error, never a
//!   silent partial.

use std::path::Path;

use serde::Deserialize;

/// One template: the file's name/description and the command with `{{name}}` holes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    pub name: String,
    pub description: String,
    pub command: String,
    /// One entry per DISTINCT placeholder, in first-appearance order. Metadata comes
    /// from the file's optional `[args]` table; a placeholder without an entry still
    /// appears here with empty metadata, so the palette can always prompt for it.
    pub args: Vec<WorkflowArg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowArg {
    pub name: String,
    pub description: String,
    /// Prefills the palette's field; `None` means the user must type something.
    pub default: Option<String>,
}

/// The on-disk shape. `args` is optional metadata -- placeholders are discovered from
/// the command text, so a file can omit it entirely.
#[derive(Deserialize)]
struct RawWorkflow {
    name: String,
    #[serde(default)]
    description: String,
    command: String,
    #[serde(default)]
    args: std::collections::BTreeMap<String, RawArg>,
}

#[derive(Deserialize)]
struct RawArg {
    #[serde(default)]
    description: String,
    default: Option<String>,
}

/// Loads every `*.toml` in `dir`, sorted by workflow name. A broken file is skipped
/// and reported in the returned error list -- one bad template must not hide the
/// rest, and a silent skip would be the looks-like-success shape.
pub fn load_dir(dir: &Path) -> (Vec<Workflow>, Vec<String>) {
    let mut workflows = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        // A missing directory is the normal fresh-machine state, not an error.
        return (workflows, errors);
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();
    for path in paths {
        let label = path.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                errors.push(format!("{label}: {error}"));
                continue;
            }
        };
        match parse(&text) {
            Ok(workflow) => workflows.push(workflow),
            Err(error) => errors.push(format!("{label}: {error}")),
        }
    }
    workflows.sort_by(|a, b| a.name.cmp(&b.name));
    (workflows, errors)
}

/// Parses one template file.
pub fn parse(text: &str) -> Result<Workflow, String> {
    let raw: RawWorkflow = toml::from_str(text).map_err(|error| error.to_string())?;
    if raw.name.trim().is_empty() {
        return Err("workflow name is empty".to_string());
    }
    let args = placeholders(&raw.command)
        .into_iter()
        .map(|name| {
            let meta = raw.args.get(&name);
            WorkflowArg {
                description: meta.map(|m| m.description.clone()).unwrap_or_default(),
                default: meta.and_then(|m| m.default.clone()),
                name,
            }
        })
        .collect();
    Ok(Workflow { name: raw.name, description: raw.description, command: raw.command, args })
}

/// The distinct `{{name}}` holes in a command, first-appearance order. A name is
/// `[A-Za-z0-9_]+`; anything else between braces is literal text and stays put.
pub fn placeholders(command: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (start, _) in command.match_indices("{{") {
        let rest = &command[start + 2..];
        let Some(end) = rest.find("}}") else { continue };
        let name = &rest[..end];
        if !name.is_empty()
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && !found.iter().any(|existing| existing == name)
        {
            found.push(name.to_string());
        }
    }
    found
}

/// Substitutes every placeholder, replace-all per name. An unresolved placeholder is
/// an ERROR, not a passthrough -- `ssh {{host}}` typed into a shell is worse than a
/// refusal the palette can show. Extra values are ignored (a caller may batch-send
/// all known args). No escaping in v1: a literal `{{` cannot be produced, documented.
pub fn render(command: &str, values: &[(String, String)]) -> Result<String, String> {
    let mut out = command.to_string();
    for (name, value) in values {
        out = out.replace(&format!("{{{{{name}}}}}"), value);
    }
    let leftover = placeholders(&out);
    if let Some(name) = leftover.first() {
        return Err(format!("placeholder {{{{{name}}}}} has no value"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_parses_and_discovers_placeholders_in_order() {
        let workflow = parse(
            r#"
name = "SSH"
description = "Open a session"
command = "ssh {{user}}@{{host}} -p {{port}} # {{host}} again"

[args]
port = { default = "22", description = "TCP port" }
"#,
        )
        .expect("parses");
        assert_eq!(workflow.name, "SSH");
        let names: Vec<_> = workflow.args.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["user", "host", "port"], "order of first appearance, deduped");
        assert_eq!(workflow.args[2].default.as_deref(), Some("22"));
        assert_eq!(workflow.args[0].default, None, "no [args] entry still prompts");
    }

    #[test]
    fn render_substitutes_every_occurrence() {
        let out = render(
            "echo {{x}} and {{x}} and {{y}}",
            &[("x".into(), "1".into()), ("y".into(), "2".into())],
        )
        .expect("renders");
        assert_eq!(out, "echo 1 and 1 and 2");
    }

    /// The rule the backlog names: a leftover hole is a refusal, never typed output.
    #[test]
    fn an_unresolved_placeholder_is_an_error_not_a_passthrough() {
        let error = render("ssh {{host}}", &[]).expect_err("must refuse");
        assert!(error.contains("{{host}}"), "{error}");
    }

    #[test]
    fn malformed_braces_are_literal_text() {
        assert!(placeholders("a {{ b }} c {{d e}} {{}}").is_empty());
        assert_eq!(placeholders("{{ok_1}}"), ["ok_1"]);
        // And render leaves them alone rather than erroring on non-placeholders.
        assert_eq!(render("a {{ b }} c", &[]).expect("renders"), "a {{ b }} c");
    }

    #[test]
    fn broken_toml_is_a_named_error() {
        assert!(parse("name = ").is_err());
        assert!(parse("command = \"x\"").is_err(), "missing name field");
        assert!(parse("name = \"\"\ncommand = \"x\"").is_err(), "empty name");
    }

    #[test]
    fn load_dir_skips_broken_files_and_reports_them() {
        let dir = std::env::temp_dir().join(format!("ruuah-wf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("a.toml"), "name = \"A\"\ncommand = \"echo a\"").expect("write");
        std::fs::write(dir.join("broken.toml"), "name = ").expect("write");
        std::fs::write(dir.join("ignored.txt"), "not toml").expect("write");
        let (workflows, errors) = load_dir(&dir);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(workflows.len(), 1);
        assert_eq!(workflows[0].name, "A");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("broken.toml:"), "{errors:?}");
    }
}
