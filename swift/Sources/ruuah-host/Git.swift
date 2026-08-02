// The panel's data source: git, read-only, one repository at a time.
//
// Every call here spawns `git` and parses its output, so every call blocks -- they are
// deliberately synchronous and the caller is responsible for keeping them off the main
// thread. Making them async here would hide that cost at each of the four call sites
// instead of stating it once.
//
// Nothing in this file mutates a repository. The panel reviews changes; it does not make
// them. That boundary is worth keeping even though it would be easy to cross, because a
// terminal that can quietly run `git checkout` on your behalf is a different and much
// more dangerous product than one that shows you a diff.

import Foundation

enum Git {
    /// Output past this is truncated with a marker rather than held in memory. A
    /// generated file's diff can be tens of megabytes and nobody reads it.
    private static let outputLimit = 4 << 20  // 4 MiB

    struct Invocation {
        let status: Int32
        let out: String
        let err: String
    }

    /// Runs git in `directory` and captures both streams.
    ///
    /// Reads the pipes on background queues before waiting. Draining after
    /// `waitUntilExit` deadlocks the moment output exceeds the pipe buffer (64 KiB),
    /// which a real diff passes immediately -- the classic shape of "works on a small
    /// repo, hangs on a real one".
    static func run(_ arguments: [String], in directory: String) -> Invocation {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/git")
        process.arguments = arguments
        process.currentDirectoryURL = URL(fileURLWithPath: directory)
        // A pager would never exit, and locale-dependent output is not parseable.
        var environment = ProcessInfo.processInfo.environment
        environment["GIT_PAGER"] = "cat"
        environment["GIT_TERMINAL_PROMPT"] = "0"
        environment["LC_ALL"] = "C"
        process.environment = environment

        let outPipe = Pipe()
        let errPipe = Pipe()
        process.standardOutput = outPipe
        process.standardError = errPipe

        var outData = Data()
        var errData = Data()
        let group = DispatchGroup()
        let queue = DispatchQueue(label: "ruuah.git.drain", attributes: .concurrent)
        queue.async(group: group) { outData = outPipe.fileHandleForReading.readDataToEndOfFile() }
        queue.async(group: group) { errData = errPipe.fileHandleForReading.readDataToEndOfFile() }

        do {
            try process.run()
        } catch {
            return Invocation(status: -1, out: "", err: "could not run git: \(error)")
        }
        process.waitUntilExit()
        group.wait()

        var out = String(decoding: outData.prefix(outputLimit), as: UTF8.self)
        if outData.count > outputLimit {
            out += "\n... truncated at \(outputLimit / (1 << 20)) MiB ...\n"
        }
        return Invocation(
            status: process.terminationStatus, out: out,
            err: String(decoding: errData.prefix(64 << 10), as: UTF8.self))
    }

    /// The work tree root containing `directory`, or nil when it is not in one.
    static func repositoryRoot(containing directory: String) -> String? {
        let result = run(["rev-parse", "--show-toplevel"], in: directory)
        guard result.status == 0 else { return nil }
        let root = result.out.trimmingCharacters(in: .whitespacesAndNewlines)
        return root.isEmpty ? nil : root
    }

    /// Changed files in the work tree, staged and unstaged, plus untracked ones.
    ///
    /// Line counts come from `--numstat` against HEAD for tracked files; an untracked
    /// file has no pre-image, so its additions are its line count and it is reported as
    /// entirely added. A binary file reports `-` for both, which becomes 0/0.
    static func changedFiles(root: String) -> Result<[ChangedFile], PanelFailure> {
        let status = run(["status", "--porcelain=v1", "-z", "--untracked-files=all"], in: root)
        guard status.status == 0 else {
            return .failure(PanelFailure(status.err.isEmpty ? "git status failed" : status.err))
        }

        var counts = numstat(root: root)
        var files: [ChangedFile] = []
        for entry in parsePorcelain(status.out) {
            let count = counts[entry.path] ?? untrackedCount(root: root, path: entry.path, status: entry.status)
            counts[entry.path] = count
            files.append(
                ChangedFile(
                    path: entry.path, status: entry.status,
                    additions: count.additions, deletions: count.deletions))
        }
        return .success(files)
    }

    /// The unified diff for one path. Untracked files are diffed against /dev/null, the
    /// only way to see the contents of a file git is not yet tracking.
    static func diff(root: String, path: String, untracked: Bool) -> Result<String, PanelFailure> {
        let result =
            untracked
            ? run(["diff", "--no-index", "--no-color", "--", "/dev/null", path], in: root)
            : run(["diff", "HEAD", "--no-color", "--", path], in: root)
        // `--no-index` exits 1 when the files differ, which is the normal case here.
        guard result.status == 0 || (untracked && result.status == 1) else {
            return .failure(PanelFailure(result.err.isEmpty ? "git diff failed" : result.err))
        }
        return .success(result.out)
    }

    // MARK: parsing

    private struct Entry {
        let status: String
        let path: String
    }

    /// `XY <path>NUL`, with renames carrying `XY <new>NUL<old>NUL`.
    ///
    /// The NUL form is not a preference: porcelain v1 without `-z` quotes and escapes
    /// paths containing spaces or non-ASCII bytes, and un-escaping that correctly is a
    /// parser nobody should write. With `-z` the bytes are literal.
    private static func parsePorcelain(_ text: String) -> [Entry] {
        var entries: [Entry] = []
        var fields = text.split(separator: "\0", omittingEmptySubsequences: false).map(String.init)
        if fields.last?.isEmpty == true { fields.removeLast() }

        var index = 0
        while index < fields.count {
            let field = fields[index]
            index += 1
            guard field.count > 3 else { continue }
            let status = String(field.prefix(2))
            let path = String(field.dropFirst(3))
            // A rename's ORIGINAL path follows in its own field; consumed, not shown.
            if status.contains("R") || status.contains("C") { index += 1 }
            entries.append(Entry(status: status, path: path))
        }
        return entries
    }

    private static func numstat(root: String) -> [String: (additions: Int, deletions: Int)] {
        let result = run(["diff", "--numstat", "-z", "HEAD"], in: root)
        guard result.status == 0 else { return [:] }
        var counts: [String: (additions: Int, deletions: Int)] = [:]
        // `-z` numstat is `adds\tdels\tpathNUL`, and for renames `adds\tdels\tNUL old NUL new NUL`.
        for record in result.out.split(separator: "\0", omittingEmptySubsequences: true) {
            let parts = record.split(separator: "\t", omittingEmptySubsequences: false)
            guard parts.count >= 3 else { continue }
            let path = String(parts[2])
            guard !path.isEmpty else { continue }
            counts[path] = (Int(parts[0]) ?? 0, Int(parts[1]) ?? 0)
        }
        return counts
    }

    private static func untrackedCount(
        root: String, path: String, status: String
    ) -> (additions: Int, deletions: Int) {
        guard status.hasPrefix("??") else { return (0, 0) }
        let url = URL(fileURLWithPath: root).appendingPathComponent(path)
        guard let data = try? Data(contentsOf: url), data.count < outputLimit else { return (0, 0) }
        // A trailing newline terminates the last line rather than starting a new one.
        var lines = data.reduce(into: 0) { total, byte in if byte == 0x0A { total += 1 } }
        if data.last != 0x0A, !data.isEmpty { lines += 1 }
        return (lines, 0)
    }
}
