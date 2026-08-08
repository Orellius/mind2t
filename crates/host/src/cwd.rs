//! Purpose: turn a raw OSC 7 report into a path the history store can key by.
//! Public surface (crate): `normalize`.
//! Why this file: the CORE stores the report exactly as the child sent it, and the oracle
//!   does the same -- decoding there would diverge on every path with a space in it. So
//!   the decoding has to happen somewhere above, and this is that somewhere: one place,
//!   unit-tested, rather than a percent-decode improvised in the view layer.
//! NOT responsible for: tracking the pwd (the core does), or deciding what to do with it
//!   (`suggest.rs` keys history by it).
//! Test strategy: the shapes real shells actually emit, plus the ones that would silently
//!   produce a wrong key.

/// The filesystem path a `file://` URI names, or `None` if it does not name one.
///
/// Accepts what shells really send: `file://host/path`, `file:///path` (empty host), and a
/// bare `/path` with no scheme at all -- OSC 9;9 and OSC 1337 report the last of those, and
/// the core stores every source in one field.
///
/// The HOST is discarded rather than validated. A remote host's path is not this machine's
/// path, but rejecting it would break every shell that reports its hostname (most of them),
/// and keying history by a directory that happens to share a name is a far smaller error
/// than having no history at all. Named here rather than left as a surprise.
pub(crate) fn normalize(raw: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(raw).ok()?.trim();
    if text.is_empty() {
        return None;
    }

    let path = match text.strip_prefix("file://") {
        // Everything up to the first `/` is the host; the rest, INCLUDING that slash, is
        // the path. `file:///tmp` therefore has an empty host and the path `/tmp`.
        Some(rest) => match rest.find('/') {
            Some(slash) => &rest[slash..],
            // `file://host` with no path at all names no directory.
            None => return None,
        },
        None if text.starts_with('/') => text,
        // Anything else -- a relative path, a different scheme -- is not something to key
        // history by. Better no key than a wrong one shared between directories.
        None => return None,
    };

    let decoded = percent_decode(path)?;
    if decoded.is_empty() {
        return None;
    }
    // A trailing slash is not part of the identity of a directory, and `/` itself would
    // otherwise be the one path that could not be keyed.
    let trimmed = decoded.trim_end_matches('/');
    Some(if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    })
}

/// Percent-decoding, refusing anything that is not valid UTF-8 afterwards.
///
/// Refusing rather than replacing: a path with a lossy replacement character in it would
/// key a history bucket no future report can ever produce again, so the entries written
/// under it would be invisible forever. `None` falls back to the global history, which is
/// merely less specific.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
            match u8::from_str_radix(hex, 16) {
                Ok(byte) => {
                    out.push(byte);
                    index += 3;
                    continue;
                }
                // A stray `%` that is not an escape is a literal percent, which is a legal
                // character in a filename.
                Err(_) => {}
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn a_file_uri_with_a_host_yields_the_path() {
        assert_eq!(normalize(b"file://mac.local/Users/orel"), Some("/Users/orel".into()));
    }

    #[test]
    fn an_empty_host_is_the_common_case() {
        assert_eq!(normalize(b"file:///tmp/x"), Some("/tmp/x".into()));
    }

    /// OSC 9;9 and OSC 1337 report a bare path, and the core stores every source in one
    /// field, so this is not a hypothetical shape.
    #[test]
    fn a_bare_absolute_path_is_accepted() {
        assert_eq!(normalize(b"/var/log"), Some("/var/log".into()));
    }

    /// The reason decoding cannot live in the core: the stored value keeps its escapes,
    /// and two different encodings of one directory must key the SAME history.
    #[test]
    fn percent_escapes_are_decoded() {
        assert_eq!(
            normalize(b"file:///Users/orel/My%20Code"),
            Some("/Users/orel/My Code".into())
        );
        assert_eq!(normalize(b"file:///a%2Fb"), Some("/a/b".into()));
    }

    #[test]
    fn a_literal_percent_survives() {
        assert_eq!(normalize(b"file:///tmp/100%"), Some("/tmp/100%".into()));
        assert_eq!(normalize(b"file:///tmp/%zz"), Some("/tmp/%zz".into()));
    }

    #[test]
    fn a_trailing_slash_does_not_make_a_second_directory() {
        assert_eq!(normalize(b"file:///tmp/"), normalize(b"file:///tmp"));
    }

    #[test]
    fn the_root_directory_survives_the_trim() {
        assert_eq!(normalize(b"file:///"), Some("/".into()));
    }

    #[test]
    fn nothing_useful_yields_no_key() {
        assert_eq!(normalize(b""), None);
        assert_eq!(normalize(b"   "), None);
        assert_eq!(normalize(b"file://justahost"), None);
        assert_eq!(normalize(b"relative/path"), None);
        assert_eq!(normalize(b"http://example.com/x"), None);
    }

    /// Invalid UTF-8 after decoding is refused rather than replaced: a lossy key could
    /// never be produced again, so everything filed under it would be unreachable.
    #[test]
    fn an_undecodable_path_is_refused_rather_than_mangled() {
        assert_eq!(normalize(b"file:///tmp/%FF%FE"), None);
    }

    /// THE PAIRING TEST. These are the exact bytes `shell/mind2t-integration.zsh` emitted
    /// for these directories, captured from a real zsh on 2026-07-31. The emitter and the
    /// decoder are in different languages in different files, and nothing else would
    /// notice if one of them changed its mind about encoding.
    #[test]
    fn what_our_shell_integration_actually_emits_decodes_back() {
        assert_eq!(
            normalize(b"file://mac.local/tmp/osc7probe/My%20Code"),
            Some("/tmp/osc7probe/My Code".into())
        );
        assert_eq!(
            normalize(b"file://mac.local/tmp/osc7probe/we%27re"),
            Some("/tmp/osc7probe/we're".into())
        );
    }
}
