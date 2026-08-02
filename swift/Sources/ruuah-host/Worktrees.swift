// Workspaces (S5): a git worktree per line of work, one session per workspace.
//
// THIS FILE MUTATES REPOSITORIES, and it is the only one in the app that does. `Git.swift`
// (S6) states the opposite rule for itself and means it -- reviewing a diff must never be
// able to change one. Creating a workspace cannot avoid it, so the boundary moves here and
// is drawn tightly instead:
//
//   - Exactly two mutating operations exist, `add` and `remove`, and both are reached only
//     from an explicit operator action with a confirmation in front of it.
//   - `remove` NEVER passes --force. git refuses to remove a worktree with uncommitted
//     changes, and that refusal is the feature: the whole point of a workspace is that an
//     agent is working in it, and "it had changes" is exactly when you must not delete it.
//     The refusal is surfaced verbatim rather than retried.
//   - Nothing here commits, checks out over existing work, resets, or touches the main
//     work tree's state.
//
// Everything is synchronous and blocks, like `Git.swift`; the caller keeps it off the
// main thread.

import Foundation

enum Worktrees {
    struct Worktree: Equatable {
        let path: String
        /// Short branch name, or nil for a detached HEAD.
        let branch: String?
        /// True for the repository's original work tree, which can never be removed.
        let isPrimary: Bool

        /// What the tab pill and the palette show.
        var label: String {
            branch ?? (path as NSString).lastPathComponent
        }
    }

    // MARK: reading

    /// Every worktree of the repository containing `directory`, primary first.
    static func list(containing directory: String) -> Result<[Worktree], PanelFailure> {
        guard let root = Git.repositoryRoot(containing: directory) else {
            return .failure(PanelFailure("not a git repository"))
        }
        let result = Git.run(["worktree", "list", "--porcelain"], in: root)
        guard result.status == 0 else {
            return .failure(PanelFailure(result.err.isEmpty ? "git worktree list failed" : result.err))
        }
        return .success(parseList(result.out))
    }

    /// Records are blank-line separated; `worktree <path>` opens each one, `branch
    /// refs/heads/<name>` names it, and `detached` replaces the branch line. The FIRST
    /// record is the primary work tree, which is a positional guarantee of the porcelain
    /// format rather than something derived from the paths.
    static func parseList(_ text: String) -> [Worktree] {
        var worktrees: [Worktree] = []
        var path: String?
        var branch: String?

        func flush() {
            guard let current = path else { return }
            worktrees.append(
                Worktree(path: current, branch: branch, isPrimary: worktrees.isEmpty))
            path = nil
            branch = nil
        }

        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            if line.isEmpty {
                flush()
            } else if line.hasPrefix("worktree ") {
                flush()
                path = String(line.dropFirst("worktree ".count))
            } else if line.hasPrefix("branch refs/heads/") {
                branch = String(line.dropFirst("branch refs/heads/".count))
            }
        }
        flush()
        return worktrees
    }

    // MARK: naming

    /// Rejects anything git would refuse, plus a leading `-`.
    ///
    /// The leading-dash case is not git's problem, it is ours: arguments are passed as an
    /// array so there is no shell to inject into, but a branch called `--force` would
    /// still be READ as a flag by git itself. `git check-ref-format` is the authority for
    /// the rest -- reimplementing its rules here would be a second, worse copy.
    static func validate(branch: String) -> PanelFailure? {
        guard !branch.isEmpty else { return PanelFailure("a workspace needs a name") }
        guard !branch.hasPrefix("-") else {
            return PanelFailure("a name cannot start with '-'")
        }
        guard !branch.contains("/../"), !branch.hasPrefix("../") else {
            return PanelFailure("a name cannot traverse directories")
        }
        let check = Git.run(
            ["check-ref-format", "--branch", branch], in: FileManager.default.currentDirectoryPath)
        guard check.status == 0 else {
            return PanelFailure("\"\(branch)\" is not a valid branch name")
        }
        return nil
    }

    /// Where a new worktree goes: `<parent>/<repo>-worktrees/<branch>`.
    ///
    /// A SIBLING of the repository, never inside it -- a worktree nested in its own
    /// parent's tree shows up in that parent's `git status` forever. The name matches the
    /// convention already in use by hand on this machine (`tools/ruuah-worktrees/`).
    /// Slashes in a branch name become dashes so `feature/x` stays one directory.
    static func location(root: String, branch: String) -> String {
        let url = URL(fileURLWithPath: root)
        let siblings = url.deletingLastPathComponent()
            .appendingPathComponent("\(url.lastPathComponent)-worktrees")
        return siblings.appendingPathComponent(branch.replacingOccurrences(of: "/", with: "-")).path
    }

    // MARK: mutating (see the file header)

    /// Creates a worktree for `branch`, making the branch if it does not exist.
    ///
    /// Never overwrites: an existing directory at the target is an error, because the
    /// alternative is silently adopting whatever is already there.
    static func add(root: String, branch: String) -> Result<Worktree, PanelFailure> {
        if let invalid = validate(branch: branch) { return .failure(invalid) }
        let path = location(root: root, branch: branch)
        guard !FileManager.default.fileExists(atPath: path) else {
            return .failure(PanelFailure("\(path) already exists"))
        }
        do {
            try FileManager.default.createDirectory(
                atPath: (path as NSString).deletingLastPathComponent,
                withIntermediateDirectories: true)
        } catch {
            return .failure(PanelFailure("could not create the worktrees directory: \(error)"))
        }

        // An existing branch is checked out; a new one is created. Asking first is what
        // makes both cases work without the caller having to know which it is.
        let exists =
            Git.run(["rev-parse", "--verify", "--quiet", "refs/heads/\(branch)"], in: root).status
            == 0
        let arguments =
            exists
            ? ["worktree", "add", "--", path, branch]
            : ["worktree", "add", "-b", branch, "--", path]
        let result = Git.run(arguments, in: root)
        guard result.status == 0 else {
            return .failure(PanelFailure(result.err.isEmpty ? "git worktree add failed" : result.err))
        }
        return .success(Worktree(path: path, branch: branch, isPrimary: false))
    }

    /// Removes a worktree, never with --force.
    ///
    /// git refuses when the tree has uncommitted changes and that refusal is passed
    /// straight through. Retrying with --force would delete an agent's unpushed work,
    /// which is the single most expensive thing this app could do.
    static func remove(root: String, worktree: Worktree) -> Result<Void, PanelFailure> {
        guard !worktree.isPrimary else {
            return .failure(PanelFailure("the primary work tree cannot be removed"))
        }
        let result = Git.run(["worktree", "remove", "--", worktree.path], in: root)
        guard result.status == 0 else {
            let detail = result.err.isEmpty ? "git worktree remove failed" : result.err
            return .failure(PanelFailure(detail.trimmingCharacters(in: .whitespacesAndNewlines)))
        }
        return .success(())
    }
}
