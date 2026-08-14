// The machines this operator already told OpenSSH about.
//
// A terminal that can open a pane on a remote box does not need its own inventory of
// hosts, an account model, or a settings screen. `~/.ssh/config` is where that list
// already lives, it is already correct, and it is already the thing `ssh` itself obeys.
// So this reads it and offers what it finds. Nothing here is written back; the file is
// the operator's.
//
// WHAT THIS DELIBERATELY DOES NOT READ: `IdentityFile`, and any other key material or
// credential-shaped value. It takes the alias and the three fields needed to DESCRIBE a
// row (HostName, User, Port) and nothing else. A key path in a palette subtitle is a
// secret on screen for no gain, and `ssh` resolves identities perfectly well without
// this file's help. The child is spawned as `ssh <alias>` precisely so that every
// resolution decision stays inside OpenSSH.
//
// This is a PRODUCT feature and it lives in the Swift host on purpose. The engine is a
// pure state machine and `crates/host` publishes a C ABI with an asserted export count;
// a host list carries no policy that could disagree with itself across two copies, so
// there is nothing to gain by pushing it down and an ABI surface to disturb by trying.
// The stated cost: a future Linux host would need its own parser rather than inheriting
// this one.

import Foundation

/// One concrete entry from `~/.ssh/config`: something `ssh <alias>` will connect to.
struct SSHHost: Equatable {
    /// The `Host` alias as written. This is what gets spawned, never `hostName`.
    let alias: String
    let hostName: String?
    let user: String?
    let port: Int?

    /// The row's second line: enough to tell two similar aliases apart, and no more.
    ///
    /// Falls back to the alias itself when the config says nothing else, because an
    /// empty subtitle reads as a broken row rather than as a host with no overrides.
    var summary: String {
        let target = [user.map { "\($0)@" }, hostName ?? alias].compactMap { $0 }.joined()
        guard let port, port != 22 else { return target }
        return "\(target):\(port)"
    }
}

/// Reads the OpenSSH client config the way `ssh` reads it, for the subset that names a
/// host worth offering.
///
/// Not a general ssh_config implementation and not trying to be: `Match` blocks are
/// recognised only well enough to stop attributing their keys to the wrong host, and
/// tokens like `%h` are passed through untouched because they are the child's business.
enum SSHConfig {
    static let defaultPath = NSString(string: "~/.ssh/config").expandingTildeInPath

    /// Every concrete host in the file, in the order it was written.
    ///
    /// File order is kept rather than sorted alphabetically: an ssh_config is ordered
    /// most-specific-first by convention, so the operator's own ordering is information.
    /// A missing or unreadable file is an empty list, never an error - not having an ssh
    /// config is the normal state of a machine, not a fault to report.
    static func hosts(configPath: String = defaultPath) -> [SSHHost] {
        /// Aliases in the order first seen, and their fields. Both accumulate across
        /// every included file rather than per file, because ssh resolves each keyword
        /// against ONE merged stream: a value obtained inside an `Include` wins over the
        /// same keyword later in the parent. Resolving each file separately and merging
        /// afterwards inverts that, and it inverts it only for hosts that appear twice -
        /// which is the one case anybody writes an Include for.
        var order: [String] = []
        var fields: [String: [String: String]] = [:]
        parse(path: configPath, depth: 0, order: &order, fields: &fields)
        return order.map { alias in
            let own = fields[alias] ?? [:]
            return SSHHost(
                alias: alias, hostName: own["hostname"], user: own["user"],
                port: own["port"].flatMap(Int.init))
        }
    }

    /// `Include` is followed to this depth and no further. OpenSSH's own limit is 16;
    /// this is lower because a palette does not need deep nesting and a cycle in a config
    /// file must not hang the app.
    private static let maximumIncludeDepth = 8

    private static func parse(
        path: String, depth: Int, order: inout [String], fields: inout [String: [String: String]]
    ) {
        guard depth <= maximumIncludeDepth,
            let data = FileManager.default.contents(atPath: path)
        else { return }

        /// The aliases the current `Host` line opened. A `Match` line closes them: its
        /// keys belong to a conditional block, and attributing them to the host above
        /// would put a stranger's user name on this row.
        var open: [String] = []

        for rawLine in String(decoding: data, as: UTF8.self).split(
            separator: "\n", omittingEmptySubsequences: false)
        {
            guard let (keyword, value) = directive(in: String(rawLine)) else { continue }
            switch keyword {
            case "host":
                open = words(in: value).filter(isConcrete)
                for alias in open where fields[alias] == nil {
                    fields[alias] = [:]
                    order.append(alias)
                }
            case "match":
                open = []
            case "include":
                let base = (path as NSString).deletingLastPathComponent
                for included in words(in: value).flatMap({ expand($0, relativeTo: base) }) {
                    parse(path: included, depth: depth + 1, order: &order, fields: &fields)
                }
            // First value wins, per alias per keyword - ssh's own rule. Getting it
            // backwards makes the LAST assignment win, which is a bug that appears only
            // on the one host with a duplicated key and looks like a typo until it is not.
            case "hostname", "user", "port":
                for alias in open where fields[alias]?[keyword] == nil {
                    fields[alias]?[keyword] = value
                }
            default:
                continue
            }
        }
    }

    /// Splits `keyword value` or `keyword=value`, lowercasing only the keyword.
    ///
    /// ssh_config keywords are case-insensitive and values are NOT, so a blanket
    /// lowercase would quietly rewrite user names and hostnames on case-sensitive
    /// systems. Both separators are legal and both appear in real configs.
    private static func directive(in line: String) -> (String, String)? {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty, !trimmed.hasPrefix("#") else { return nil }
        guard let split = trimmed.firstIndex(where: { $0 == "=" || $0.isWhitespace }) else {
            return nil
        }
        let keyword = trimmed[trimmed.startIndex..<split].lowercased()
        let rest = trimmed[trimmed.index(after: split)...]
            .drop { $0 == "=" || $0.isWhitespace }
        guard !keyword.isEmpty, !rest.isEmpty else { return nil }
        return (keyword, unquoted(String(rest)))
    }

    /// Whitespace-separated words, each unquoted. `Host` and `Include` both take a list.
    private static func words(in value: String) -> [String] {
        value.split(whereSeparator: { $0.isWhitespace }).map { unquoted(String($0)) }
    }

    private static func unquoted(_ value: String) -> String {
        guard value.count >= 2, value.hasPrefix("\""), value.hasSuffix("\"") else { return value }
        return String(value.dropFirst().dropLast())
    }

    /// A pattern is offerable only when it names exactly one machine.
    ///
    /// `*` and `?` are matchers and `!` is a negation; every one of them describes a RULE
    /// for other hosts rather than a host. Offering `Host *` as a row would spawn
    /// `ssh *`, which is a connection attempt to a literal asterisk.
    private static func isConcrete(_ pattern: String) -> Bool {
        !pattern.isEmpty && !pattern.contains(where: { $0 == "*" || $0 == "?" || $0 == "!" })
    }

    /// Resolves one `Include` argument to real paths: tilde, then relative to the
    /// including file's own directory, then glob.
    ///
    /// OpenSSH documents relative includes as relative to `~/.ssh` for a user config, and
    /// for the real `~/.ssh/config` the two rules give the same answer because that IS its
    /// directory. Using the parent's directory instead makes the rule hold for any config
    /// this is pointed at, which is what lets the gate parse a fixture at all. Hardcoding
    /// `~/.ssh` here meant the include was silently skipped everywhere else, and a skipped
    /// include looks exactly like a config that simply has fewer hosts in it.
    private static func expand(_ argument: String, relativeTo base: String) -> [String] {
        var path = NSString(string: argument).expandingTildeInPath
        if !path.hasPrefix("/") {
            path = base + "/" + path
        }
        guard path.contains("*") || path.contains("?") else { return [path] }
        var results = glob_t()
        defer { globfree(&results) }
        guard glob(path, 0, nil, &results) == 0 else { return [] }
        return (0..<Int(results.gl_pathc)).compactMap { index in
            results.gl_pathv[index].map { String(cString: $0) }
        }
    }
}
