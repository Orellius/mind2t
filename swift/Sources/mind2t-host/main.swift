// The minimal Swift host, slice 8. Two modes:
//
//   mind2t-host --smoke        headless: spawn a known child, poll until its pixels arrive,
//                             assert there is ink, exit 0/1. This is the CI-assertable
//                             proof that the ABI is consumable from Swift at all.
//   mind2t-host [--command X]  a window: blit polled frames, forward keys, live resize.
//
// Everything the host knows arrives through CMind2tHost -- the five mind2t_host_* calls.
// There is deliberately no second channel into the Rust side.

import AppKit
import CMind2tHost

/// Runs `body` with a borrowed C string, or with NULL when there is nothing to lend.
///
/// `withCString` has no optional form, and the alternative -- a branch per optional
/// argument -- doubles with each one. Two of them would already be four spawn sites
/// that have to stay in step.
///
/// Module-wide rather than private since the agent spawn in `Agents.swift` is a third site.
/// A second copy of a borrow helper is a second chance to get a lifetime wrong.
func withOptionalCString<R>(
    _ value: String?, _ body: (UnsafePointer<CChar>?) -> R
) -> R {
    guard let value else { return body(nil) }
    return value.withCString { body($0) }
}

func spawnHost(
    cols: UInt16, rows: UInt16, fontSize: Float, command: String?, autoDirection: Bool = false,
    config: OpaquePointer? = nil, cwd: String? = nil
) -> OpaquePointer? {
    var host: OpaquePointer?
    let result = withOptionalCString(command) { commandPointer in
        withOptionalCString(cwd) { cwdPointer in
            var options = Mind2tHostOptions(
                cols: cols, rows: rows, font_size: fontSize, command: commandPointer,
                auto_direction: autoDirection, config: config, cwd: cwdPointer)
            return mind2t_host_spawn(&options, &host)
        }
    }
    guard result == MIND2T_HOST_SUCCESS, let host else {
        FileHandle.standardError.write(Data("spawn failed: \(result)\n".utf8))
        return nil
    }
    return host
}

/// Headless proof: a child's output becomes ink, through the archive, from Swift.
func runSmoke() -> Int32 {
    guard let host = spawnHost(
        cols: 80, rows: 24, fontSize: 0, command: "printf 'MIND2T-VT-SMOKE\\n'")
    else { return 1 }
    defer { mind2t_host_free(host) }

    let deadline = Date().addingTimeInterval(10)
    var frame = Mind2tHostFrame()
    while Date() < deadline {
        guard mind2t_host_poll(host, &frame) == MIND2T_HOST_SUCCESS else {
            FileHandle.standardError.write(Data("poll failed\n".utf8))
            return 1
        }
        if frame.child_exited, let pixels = frame.pixels, !frame.drew {
            // The child is gone and its final frame is drawn. Ink = any pixel that is not
            // the background, and the background is whatever the top-left corner shows.
            let count = Int(frame.width) * Int(frame.height)
            var ink = 0
            for index in 0..<count {
                let pixel = pixels.advanced(by: index * 4)
                if pixel[0] != pixels[0] || pixel[1] != pixels[1] || pixel[2] != pixels[2] {
                    ink += 1
                }
            }
            guard ink > 0 else {
                FileHandle.standardError.write(Data("frame arrived with no ink\n".utf8))
                return 1
            }
            print("SMOKE OK: \(ink) ink pixels in \(frame.width)x\(frame.height), generation \(frame.generation)")
            return 0
        }
        usleep(10_000)
    }
    FileHandle.standardError.write(Data("no settled frame within 10s\n".utf8))
    return 1
}

/// Headless proof of the cwd-keyed ghost, end to end through the Swift seam.
///
/// This is the seam PR #15 shipped untested, and its failure is SILENT: if the OSC 7
/// event never reaches the session, `pwdRaw` stays empty, every lookup falls back to
/// global history, and the result is indistinguishable from the feature not existing.
/// So the assertion is the discriminating one -- a command run HERE must win over a
/// NEWER command run elsewhere. With the cwd lost, the newer one wins and this fails.
func runHistorySmoke() -> Int32 {
    let directoryA = "/tmp/mind2t-cwd-smoke-a"
    let directoryB = "/tmp/mind2t-cwd-smoke-b"

    /// Spawns a child whose only job is to report a directory, and returns the session
    /// once that report has been drained. Real OSC 7 bytes down a real pty -- the point
    /// is to exercise the event path, not to assign to `pwdRaw` directly.
    func sessionReporting(_ directory: String) -> Session? {
        guard let session = Session(
            command: "printf '\\033]7;file://localhost\(directory)\\033\\\\'",
            cols: 80, rows: 24, fontSize: 0, autoDirection: false, title: "cwd-smoke")
        else { return nil }

        let deadline = Date().addingTimeInterval(10)
        while Date() < deadline {
            _ = session.poll()
            for event in session.drainEvents() {
                if case .pwd = event { return session }
            }
            if session.exited, !session.pwdRaw.isEmpty { return session }
            usleep(10_000)
        }
        FileHandle.standardError.write(
            Data("no OSC 7 event for \(directory) within 10s\n".utf8))
        return nil
    }

    let storePath = "/tmp/mind2t-cwd-smoke-history"
    try? FileManager.default.removeItem(atPath: storePath)
    var store: OpaquePointer?
    guard storePath.withCString({ mind2t_history_load($0, &store) }) == MIND2T_HOST_SUCCESS
    else {
        FileHandle.standardError.write(Data("history store failed to load\n".utf8))
        return 1
    }

    guard let inA = sessionReporting(directoryA) else { return 1 }
    guard String(decoding: inA.pwdRaw, as: UTF8.self).contains(directoryA) else {
        FileHandle.standardError.write(
            Data(("session reported \(String(decoding: inA.pwdRaw, as: UTF8.self)),"
                + " expected \(directoryA)\n").utf8))
        return 1
    }
    inA.recordCommand(store, command: "echo alpha-here")

    guard let inB = sessionReporting(directoryB) else { return 1 }
    inB.recordCommand(store, command: "echo beta-here")

    // Back where alpha ran. beta is NEWER, so a lookup that lost the directory returns it.
    guard let backInA = sessionReporting(directoryA) else { return 1 }
    let suggestion = backInA.suggestion(store, for: "echo ")

    guard suggestion == "echo alpha-here" else {
        FileHandle.standardError.write(
            Data("""
                cwd-keyed history FAILED: suggested \(suggestion ?? "nothing"), \
                expected 'echo alpha-here'. The session's reported directory was \
                '\(String(decoding: backInA.pwdRaw, as: UTF8.self))' -- if that is empty, \
                the OSC 7 event never reached the Swift layer and every lookup is global.

                """.utf8))
        return 1
    }

    // The control: with no directory, the newest match anywhere is the right answer.
    // Without this, the assertion above would also pass on a store holding only alpha.
    guard let globalSession = Session(
        command: "true", cols: 80, rows: 24, fontSize: 0, autoDirection: false,
        title: "cwd-smoke-control"), globalSession.pwdRaw.isEmpty,
        globalSession.suggestion(store, for: "echo ") == "echo beta-here"
    else {
        FileHandle.standardError.write(
            Data("control failed: a session with no cwd must fall back to the newest\n".utf8))
        return 1
    }

    print("HISTORY SMOKE OK: in \(directoryA) the ghost suggests 'echo alpha-here'; "
        + "with no cwd it suggests the newer 'echo beta-here'")
    return 0
}

/// Headless proof of the web-panel bridge, both directions, with no window.
///
/// The seam this asserts is the one that fails SILENTLY (SCAR-004): WKWebView does not
/// throw when a script message handler was never registered, and evaluateJavaScript
/// against a document whose receiver is missing reports a JS exception nobody reads. So
/// a panel that is completely disconnected looks exactly like a panel that is working
/// and simply has nothing to show. The probe is therefore a ROUND TRIP with a nonce: the
/// host posts `ping`, the panel's bridge module answers `pong` with the same nonce, and
/// only receiving that nonce back proves both directions of the channel carried data.
///
/// Run against a built `web/dist` via `--web-dir`, since the bare CLI binary has no
/// resource bundle. The control is `--smoke-panel-control`, which loads a document with
/// the bridge stripped out and must NOT get its nonce back.
func runPanelSmoke(webDir: String?, control: Bool) -> Int32 {
    guard var url = WebPanel.documentURL(override: webDir) else {
        FileHandle.standardError.write(
            Data("panel document not found; build web/ or pass --web-dir\n".utf8))
        return 1
    }

    // The control's document is the real one with the host's entry point removed --
    // the single line that makes the bridge exist. Everything else about the load,
    // the handler registration and the probe is identical, so a pass here would mean
    // the assertion is not measuring the bridge at all.
    if control {
        guard let html = try? String(contentsOf: url, encoding: .utf8) else {
            FileHandle.standardError.write(Data("could not read the panel document\n".utf8))
            return 1
        }
        let broken = html.replacingOccurrences(
            of: "window.__mind2tReceive=", with: "window.__mind2tDisconnected=")
        guard broken != html else {
            FileHandle.standardError.write(
                Data("control could not find the receiver to remove; the bundle changed shape\n".utf8))
            return 1
        }
        let path = NSTemporaryDirectory() + "mind2t-panel-control.html"
        guard (try? broken.write(toFile: path, atomically: true, encoding: .utf8)) != nil else {
            FileHandle.standardError.write(Data("could not write the control document\n".utf8))
            return 1
        }
        url = URL(fileURLWithPath: path)
    }

    guard let panel = WebPanel(documentURL: url) else {
        FileHandle.standardError.write(Data("panel could not be constructed\n".utf8))
        return 1
    }
    // Off-screen but in a real window: WKWebView does not run a document that is in no
    // window at all, and this must exercise the same path the app uses.
    let window = NSWindow(
        contentRect: NSRect(x: 0, y: 0, width: 800, height: 600),
        styleMask: [.titled], backing: .buffered, defer: false)
    panel.frame = NSRect(x: 0, y: 0, width: 800, height: 600)
    window.contentView?.addSubview(panel)

    // Before anything: the document cannot be on screen yet, and that is the assertion
    // that keeps WebKit's white pre-paint off a dark terminal. A panel that revealed its
    // web view at construction flashes white across its whole area while WebKit starts
    // up -- obvious on the first open, brief once the web process is warm, and always
    // drawn (operator-reported, 2026-08-02).
    guard !panel.isContentVisible else {
        FileHandle.standardError.write(
            Data("PANEL SMOKE FAILED: the web view was visible before the document was\n".utf8))
        return 1
    }

    let nonce = UUID().uuidString
    var pongedWith: String?
    var ready = false
    var errors: [String] = []
    panel.onProtocolError = { errors.append($0) }
    panel.onMessage = { message in
        switch message {
        case .ready: ready = true
        case .pong(let echoed): pongedWith = echoed
        default: break
        }
    }
    panel.post(.ping(nonce: nonce))

    // Waits for BOTH signals. `ready` comes from the React layer once it has mounted and
    // `pong` from the bridge module, so requiring both means the document is proven to
    // have executed AND the channel is proven to carry data. Waiting only for the pong
    // would pass against a bundle whose UI never rendered at all.
    let deadline = Date().addingTimeInterval(15)
    while Date() < deadline, pongedWith == nil || !ready {
        RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.02))
    }

    if control {
        // The control is only worth something if it fails for the RIGHT reason. Removing
        // the receiver does not stop React mounting, so a correct control still reports
        // `ready` -- and demanding that here is what distinguishes "the bridge is dead"
        // from "the document never loaded", which is how this control passed VACUOUSLY
        // on its first run (a navigation-policy bug refused the document outright, and
        // the missing nonce proved nothing at all).
        guard ready else {
            FileHandle.standardError.write(
                Data("""
                    CONTROL INVALID: the document never reported ready, so a missing nonce \
                    says nothing about the bridge. errors=\
                    \(errors.isEmpty ? "none" : errors.joined(separator: "; "))

                    """.utf8))
            return 1
        }
        guard pongedWith == nil else {
            FileHandle.standardError.write(
                Data("CONTROL FAILED: a document with no receiver still answered the probe\n".utf8))
            return 1
        }
        print(
            "PANEL CONTROL OK: the document loaded and reported ready, and with the "
                + "receiver removed the nonce never came back")
        return 0
    }

    guard let pongedWith else {
        FileHandle.standardError.write(
            Data("""
                PANEL SMOKE FAILED: no pong within 15s. ready=\(ready), \
                errors=\(errors.isEmpty ? "none" : errors.joined(separator: "; "))

                """.utf8))
        return 1
    }
    guard pongedWith == nonce else {
        FileHandle.standardError.write(
            Data("PANEL SMOKE FAILED: echoed \(pongedWith), sent \(nonce)\n".utf8))
        return 1
    }
    guard ready else {
        FileHandle.standardError.write(
            Data("PANEL SMOKE FAILED: the panel answered the probe but never mounted\n".utf8))
        return 1
    }
    guard panel.isContentVisible else {
        FileHandle.standardError.write(
            Data("PANEL SMOKE FAILED: the document reported ready but stayed hidden\n".utf8))
        return 1
    }
    guard errors.isEmpty else {
        FileHandle.standardError.write(
            Data("PANEL SMOKE FAILED: bridge errors: \(errors.joined(separator: "; "))\n".utf8))
        return 1
    }
    print(
        "PANEL SMOKE OK: hidden until ready, then shown; the panel mounted and nonce "
            + "\(nonce) crossed to it and back")
    return 0
}

/// Headless proof of the workspace layer (S5), against a real repository.
///
/// The assertion that carries the weight is the REFUSAL. `Worktrees.remove` never passes
/// --force, so git declines to delete a worktree with uncommitted changes, and that is
/// the one behaviour standing between this feature and destroying an agent's unpushed
/// work. A remove that quietly succeeded on a dirty tree would look identical to a
/// correct one from the outside: the session closes, the tab disappears, and the loss is
/// discovered later. So the test dirties the tree, requires the removal to FAIL and the
/// directory to survive, then cleans it and requires the same call to succeed.
func runWorktreeSmoke() -> Int32 {
    let base = NSTemporaryDirectory() + "mind2t-worktree-smoke-\(ProcessInfo.processInfo.processIdentifier)"
    let root = base + "/repo"
    try? FileManager.default.removeItem(atPath: base)
    defer { try? FileManager.default.removeItem(atPath: base) }

    func fail(_ message: String) -> Int32 {
        FileHandle.standardError.write(Data("WORKTREE SMOKE FAILED: \(message)\n".utf8))
        return 1
    }

    do {
        try FileManager.default.createDirectory(
            atPath: root, withIntermediateDirectories: true)
    } catch {
        return fail("could not create \(root): \(error)")
    }
    // A repository with one commit: `git worktree add` has nothing to point at otherwise.
    for arguments in [
        ["init", "--initial-branch=main"],
        ["config", "user.email", "smoke@mind2t.local"],
        ["config", "user.name", "mind2t smoke"],
    ] {
        let result = Git.run(arguments, in: root)
        guard result.status == 0 else { return fail("git \(arguments[0]): \(result.err)") }
    }
    guard (try? "seed\n".write(toFile: root + "/seed.txt", atomically: true, encoding: .utf8))
        != nil
    else { return fail("could not seed the repository") }
    for arguments in [["add", "-A"], ["commit", "-m", "seed"]] {
        let result = Git.run(arguments, in: root)
        guard result.status == 0 else { return fail("git \(arguments[0]): \(result.err)") }
    }

    // Create.
    let branch = "smoke-workspace"
    guard case .success(let worktree) = Worktrees.add(root: root, branch: branch) else {
        return fail("worktree add did not succeed")
    }
    guard FileManager.default.fileExists(atPath: worktree.path) else {
        return fail("worktree add reported success but \(worktree.path) does not exist")
    }
    // The sibling convention, asserted rather than assumed: a worktree nested inside its
    // own parent's tree pollutes that parent's status forever.
    guard !worktree.path.hasPrefix(root + "/") else {
        return fail("the worktree landed INSIDE the repository: \(worktree.path)")
    }

    // List, through the real porcelain parser.
    guard case .success(let trees) = Worktrees.list(containing: root) else {
        return fail("worktree list did not succeed")
    }
    guard trees.first?.isPrimary == true else {
        return fail("the first record must be the primary work tree")
    }
    guard let listed = trees.first(where: { $0.branch == branch }) else {
        return fail("the new worktree is missing from the list: \(trees.map(\.label))")
    }
    guard !listed.isPrimary else { return fail("a secondary worktree was marked primary") }

    // The refusal. This is the assertion the whole file exists for.
    guard (try? "unsaved\n".write(
        toFile: listed.path + "/work-in-progress.txt", atomically: true, encoding: .utf8)) != nil
    else { return fail("could not dirty the worktree") }
    let refused = Worktrees.remove(root: root, worktree: listed)
    guard case .failure(let refusal) = refused else {
        return fail(
            "A DIRTY WORKTREE WAS REMOVED. This is the case that destroys unpushed work; "
                + "remove() must never pass --force.")
    }
    guard FileManager.default.fileExists(atPath: listed.path) else {
        return fail("removal reported failure but the directory is gone anyway")
    }

    // And the other direction, so the refusal above is not simply "remove never works".
    try? FileManager.default.removeItem(atPath: listed.path + "/work-in-progress.txt")
    guard case .success = Worktrees.remove(root: root, worktree: listed) else {
        return fail("a clean worktree could not be removed either, so the refusal proves nothing")
    }
    guard !FileManager.default.fileExists(atPath: listed.path) else {
        return fail("removal reported success but the directory survives")
    }

    // The primary is never removable, whatever the caller asks.
    guard case .failure = Worktrees.remove(root: root, worktree: trees[0]) else {
        return fail("the primary work tree was removed")
    }

    print(
        "WORKTREE SMOKE OK: created \(listed.path), removal REFUSED while dirty "
            + "(\(refusal.description.split(separator: "\n").first ?? "")), succeeded once clean")
    return 0
}


/// Headless proof that an agent really reaches a pane THROUGH THE SWIFT SEAM, and that the
/// approval guard survives the trip.
///
/// The Rust side is gated in `crates/host/tests/agent_abi.rs`. This is the half that suite
/// cannot see: `Agent.all`, the argv borrow, `Session.agent`, and the failure classification
/// the UI shows. Every one of those can be wrong while the C surface underneath is perfect,
/// and the way they would be wrong is silent - a dangling argv pointer, a `Result` mapped to
/// the wrong case, a menu that lists nothing.
///
/// A FAKE agent on PATH, not a real one: an automated gate must not start authenticated agent
/// processes on somebody's machine every time it builds. `opencode` is a real registry entry,
/// and our directory goes first on PATH so this behaves identically on a machine that has the
/// real one installed.
func runAgentSmoke() -> Int32 {
    func fail(_ why: String) -> Int32 {
        FileHandle.standardError.write(Data("agent smoke: \(why)\n".utf8))
        return 1
    }

    let directory = "/tmp/mind2t-agent-smoke-\(getpid())"
    let binary = "\(directory)/opencode"
    try? FileManager.default.createDirectory(
        atPath: directory, withIntermediateDirectories: true)
    guard
        FileManager.default.createFile(
            atPath: binary,
            contents: Data("#!/bin/sh\nprintf 'AGENT-SMOKE-UP\\n'\nexec cat\n".utf8),
            attributes: [.posixPermissions: 0o755])
    else { return fail("could not write the fake agent at \(binary)") }
    defer { try? FileManager.default.removeItem(atPath: directory) }
    setenv("PATH", "\(directory):\(ProcessInfo.processInfo.environment["PATH"] ?? "")", 1)

    // The registry, as a menu would read it.
    let agents = Agent.all()
    guard !agents.isEmpty else { return fail("the registry came back empty") }
    guard let opencode = agents.first(where: { $0.id == "opencode" }) else {
        return fail("opencode is not in the registry")
    }
    guard opencode.path == binary else {
        return fail("resolved \(opencode.path ?? "nothing"), expected our fake at \(binary)")
    }
    guard agents.contains(where: { $0.typeAfterLaunch }),
        agents.contains(where: { !$0.typeAfterLaunch })
    else { return fail("both prompt strategies did not survive the trip into Swift") }

    // THE CONTROL, FIRST. A bypass must be refused rather than stripped, and it must be
    // refused with the FLAG NAMED - a Swift layer that mapped every failure onto one case
    // would still "work" and would tell the operator nothing.
    switch Session.agent(
        opencode, argv: ["--yolo"], cols: 80, rows: 24, fontSize: 0, autoDirection: false)
    {
    case .success: return fail("a --yolo launch was allowed through the Swift seam")
    case .failure(.refused(let flag, let at)):
        guard flag == "--yolo", at == 0 else {
            return fail("refused, but named \(flag) at \(at)")
        }
    case .failure(let other): return fail("refused for the wrong reason: \(other.summary)")
    }

    // And the launch itself, with the operator's own near-miss flag riding along - so the
    // refusal above is about the FLAG and not about this path being broken.
    let launched = Session.agent(
        opencode, argv: ["--autosave"], cols: 80, rows: 24, fontSize: 0, autoDirection: false)
    guard case .success(let session) = launched else {
        guard case .failure(let why) = launched else { return fail("unreachable") }
        return fail("the agent did not launch: \(why.summary)")
    }
    defer { session.close() }

    let deadline = Date().addingTimeInterval(15)
    while Date() < deadline {
        _ = session.poll()
        // Read off the TYPED GRID, never off the byte stream: that is the whole wedge, and a
        // byte-scraping version of this could be fooled by an agent that repaints its banner.
        if (0..<24).contains(where: { session.rowText(UInt16($0), semantic: UInt8(MIND2T_TEXT_ALL)).contains("AGENT-SMOKE-UP") }) {
            print(
                "AGENT SMOKE OK: \(agents.count) agents listed, --yolo refused by name, "
                    + "\(opencode.name) up in a pane from \(binary)")
            return 0
        }
        usleep(10_000)
    }
    return fail("the agent never appeared on the grid within 15s")
}

/// Chrome geometry: the pane and the docked sidebar must TILE the content exactly.
///
/// Pure arithmetic, no window and no views, which is the whole reason `ChromeLayout` was
/// extracted from `layoutChrome` in the first place. The failure it guards is SILENT: a
/// pane that keeps its full width while a sidebar is docked simply draws underneath it,
/// and the terminal looks fine until you notice the right-hand columns are covered.
///
/// The narrow cases are the discriminating ones. A sidebar computed as a CONSTANT
/// (`x = width - 260`) tiles perfectly at 1120 and overlaps the pane at 300, so a gate
/// that only tests a comfortable window passes on the wrong implementation.
func runChromeSmoke() -> Int32 {
    func fail(_ why: String) -> Int32 {
        FileHandle.standardError.write(Data("chrome smoke: \(why)\n".utf8))
        return 1
    }

    let tabHeight = TabBarView.height
    let requested = SidebarView.preferredWidth
    // 1120 is the shipped default. 380 puts the pane just above its floor. 300 forces the
    // floor to win. 100 is narrower than the floor itself, where the pane must clamp
    // rather than the sidebar going negative.
    let widths: [CGFloat] = [1120, 800, 380, 300, 200, 100]
    let heights: [CGFloat] = [700, 200, tabHeight, 10]

    for width in widths {
        for height in heights {
            let size = NSSize(width: width, height: height)
            let layout = ChromeLayout.compute(
                content: size, tabHeight: tabHeight, sidebarWidth: requested)
            guard let sidebar = layout.sidebar else {
                return fail("no sidebar rect at \(width)x\(height) with a width requested")
            }
            let pane = layout.pane

            // Horizontal: no gap, no overlap, and the pair spans the content exactly.
            if pane.minX != 0 {
                return fail("pane starts at \(pane.minX), not 0, at \(width)x\(height)")
            }
            if pane.maxX != sidebar.minX {
                return fail(
                    "pane ends at \(pane.maxX) and sidebar starts at \(sidebar.minX)"
                        + " at \(width)x\(height) -- "
                        + (pane.maxX > sidebar.minX ? "OVERLAP" : "GAP"))
            }
            if sidebar.maxX != width {
                return fail(
                    "sidebar ends at \(sidebar.maxX), content is \(width) wide"
                        + " at \(width)x\(height)")
            }
            if pane.width < 0 || sidebar.width < 0 {
                return fail(
                    "negative width at \(width)x\(height):"
                        + " pane \(pane.width), sidebar \(sidebar.width)")
            }
            // The floor is a floor, not a suggestion -- except where the content itself
            // is narrower than the floor, in which case the pane takes everything and the
            // sidebar is zero-width. Both are correct; a pane BELOW the floor while the
            // sidebar still has width is not.
            if pane.width < ChromeLayout.minimumPaneWidth && sidebar.width > 0 {
                return fail(
                    "pane \(pane.width) is under the \(ChromeLayout.minimumPaneWidth) floor"
                        + " while the sidebar still holds \(sidebar.width) at \(width)x\(height)")
            }

            // Vertical: the strip owns the top band, the pane and the sidebar share the
            // rest, and nothing is left over.
            let below = max(0, height - tabHeight)
            if layout.tabBar.maxY != height {
                return fail("tab strip ends at \(layout.tabBar.maxY), content is \(height) tall")
            }
            if pane.minY != 0 || pane.maxY != below {
                return fail(
                    "pane spans \(pane.minY)..\(pane.maxY), expected 0..\(below)"
                        + " at \(width)x\(height)")
            }
            if sidebar.minY != pane.minY || sidebar.maxY != pane.maxY {
                return fail(
                    "sidebar spans \(sidebar.minY)..\(sidebar.maxY) but the pane spans"
                        + " \(pane.minY)..\(pane.maxY) at \(width)x\(height)")
            }
        }
    }

    // THE CONTROL. Without it every assertion above could be satisfied by a layout that
    // always docks, and "undocked" would be untested while looking covered.
    let undocked = ChromeLayout.compute(
        content: NSSize(width: 1120, height: 700), tabHeight: tabHeight, sidebarWidth: nil)
    if undocked.sidebar != nil {
        return fail("a sidebar rect exists with no width requested")
    }
    if undocked.pane.width != 1120 {
        return fail(
            "undocked pane is \(undocked.pane.width) wide, not the full 1120 -- the pane"
                + " is still paying for a sidebar that is not there")
    }

    // The row limit this file documents, asserted rather than trusted. A sidebar 700pt
    // tall minus a 26pt header, at 46pt a row, holds 14.
    let view = SidebarView(frame: NSRect(x: 0, y: 0, width: requested, height: 700))
    let capacity = view.visibleRowCapacity
    if capacity != 14 {
        return fail("row capacity is \(capacity) at 700pt, expected 14")
    }
    if SidebarView(frame: .zero).visibleRowCapacity != 0 {
        return fail("a zero-height sidebar claims it can show rows")
    }

    print(
        "CHROME SMOKE OK: pane and sidebar tile exactly at \(widths.count)x\(heights.count)"
            + " sizes down to 100pt, the \(Int(ChromeLayout.minimumPaneWidth))pt pane floor"
            + " wins over the \(Int(requested))pt request, undocked gives the pane all"
            + " 1120, capacity \(capacity) rows at 700pt")
    return 0
}

/// `--smoke-ssh`: parses a fixture ssh_config and asserts what the palette will offer.
///
/// It writes its own config tree rather than reading the operator's, for two reasons. The
/// obvious one is determinism. The other is that `~/.ssh` is a credential directory and a
/// gate that reads it would print its contents into CI output the first time it failed.
///
/// The discriminating cases are the ones a naive line-splitter gets wrong: a wildcard
/// pattern is not a host, a second block for the same alias must LOSE, a `Match` block's
/// keys must not stick to the host above it, and an `Include` must be followed in place.
/// A parser that simply collected every `Host` word would pass none of them, and a parser
/// that collected the LAST value for each keyword would pass everything except one line.
func runSSHSmoke() -> Int32 {
    func fail(_ why: String) -> Int32 {
        FileHandle.standardError.write(Data("ssh smoke: \(why)\n".utf8))
        return 1
    }

    let root = NSTemporaryDirectory() + "mind2t-ssh-smoke-\(getpid())"
    let includes = root + "/includes"
    defer { try? FileManager.default.removeItem(atPath: root) }
    do {
        try FileManager.default.createDirectory(
            atPath: includes, withIntermediateDirectories: true)
        // Written with real tabs and a `Key=Value` line because both appear in configs
        // people actually have, and both are places a hand-rolled split goes wrong.
        try """
            # a comment, then a blank line

            Host bastion
            \tHostName bastion.example.net
            \tUser orel
            \tPort 2222

            Host *.internal !secret.internal
            \tUser nobody

            Host=compact
            \tHostName=10.0.0.9

            Include includes/*.conf

            Host bastion
            \tUser loser
            \tPort 9999

            Host tail
            \tHostName tail.example.net

            Match host tail
            \tUser stranger

            Host *
            \tUser everyone
            """.write(toFile: root + "/config", atomically: true, encoding: .utf8)
        try """
            Host inner
            \tHostName inner.example.net
            \tUser innerguy
            \tPort 22
            """.write(toFile: includes + "/extra.conf", atomically: true, encoding: .utf8)
    } catch {
        return fail("could not write the fixture: \(error)")
    }

    let hosts = SSHConfig.hosts(configPath: root + "/config")

    // Order AND membership in one assertion. `*.internal` and `!secret.internal` are
    // matchers, `*` is a matcher, and `inner` proves the Include was followed at the
    // point it appeared rather than appended at the end.
    let aliases = hosts.map(\.alias)
    if aliases != ["bastion", "compact", "inner", "tail"] {
        return fail("aliases were \(aliases), expected [bastion, compact, inner, tail]")
    }

    guard let bastion = hosts.first(where: { $0.alias == "bastion" }) else {
        return fail("bastion missing after the alias check passed")
    }
    // The second `Host bastion` block says loser/9999. ssh takes the FIRST value.
    if bastion.user != "orel" || bastion.port != 2222 {
        return fail(
            "bastion resolved to user \(bastion.user ?? "nil") port"
                + " \(bastion.port.map(String.init) ?? "nil") -- the later duplicate block won,"
                + " so first-value-wins is inverted")
    }
    if bastion.hostName != "bastion.example.net" {
        return fail("bastion hostname is \(bastion.hostName ?? "nil")")
    }

    guard let compact = hosts.first(where: { $0.alias == "compact" }),
        compact.hostName == "10.0.0.9"
    else { return fail("the Key=Value form did not parse") }

    guard let inner = hosts.first(where: { $0.alias == "inner" }),
        inner.user == "innerguy", inner.hostName == "inner.example.net"
    else { return fail("the included host parsed without its fields") }

    guard let tail = hosts.first(where: { $0.alias == "tail" }) else {
        return fail("tail missing")
    }
    // Two separate leaks land on this one host: the `Match` block directly below it, and
    // the trailing `Host *`. Either one attaching would put a stranger's name on the row.
    if tail.user != nil {
        return fail(
            "tail picked up user \(tail.user ?? "") -- a Match block or a wildcard Host"
                + " attached its keys to the host above it")
    }

    // The subtitle is what the operator actually reads, so it is asserted, not assumed.
    if bastion.summary != "orel@bastion.example.net:2222" {
        return fail("bastion summary is \(bastion.summary)")
    }
    if inner.summary != "innerguy@inner.example.net" {
        return fail("inner summary is \(inner.summary) -- port 22 should be left unsaid")
    }

    // THE CONTROL. Not having an ssh config is the normal state of a machine, and it must
    // read as an empty list rather than as a failure. Without this the whole feature could
    // be crashing on a fresh machine while every assertion above passed.
    if !SSHConfig.hosts(configPath: root + "/does-not-exist").isEmpty {
        return fail("a missing config produced hosts")
    }

    print(
        "SSH SMOKE OK: \(hosts.count) hosts from a config with an Include, wildcard and"
            + " negation patterns refused, a duplicate block lost to first-value-wins,"
            + " a Match block stayed off the host above it, missing config gave nothing")
    return 0
}

/// `--smoke-ssh-write`: the connection form's writer, against a fixture config.
///
/// This gate guards a MUTATION of a file the app did not author, in a credential
/// directory, so its assertions are about damage rather than about features. In order of
/// how bad the failure is: a newline smuggled through any field would append a second
/// block to the operator's config (a `Host *` stanza applies to every machine they own);
/// a rewrite instead of an append would put every existing host at the mercy of this
/// code; a duplicate alias would produce a saved host that behaves as though its settings
/// were ignored, because ssh takes the first value it obtains.
///
/// It also asserts the file mode, because `~/.ssh/config` created world-readable is a
/// problem that surfaces much later as ssh refusing a key with an error naming the key.
func runSSHWriteSmoke() -> Int32 {
    func fail(_ why: String) -> Int32 {
        FileHandle.standardError.write(Data("ssh write smoke: \(why)\n".utf8))
        return 1
    }

    let root = NSTemporaryDirectory() + "mind2t-ssh-write-smoke-\(getpid())"
    let path = root + "/config"
    defer { try? FileManager.default.removeItem(atPath: root) }
    let existing = """
        Host already-here
        \tHostName first.example.net
        \tUser first
        """
    do {
        try FileManager.default.createDirectory(
            atPath: root, withIntermediateDirectories: true)
        // Deliberately written with NO trailing newline: appending to a file whose last
        // line is unterminated is the case that joins two directives into one.
        try existing.write(toFile: path, atomically: true, encoding: .utf8)
    } catch {
        return fail("could not write the fixture: \(error)")
    }

    // THE INJECTION CASE. Refused, not sanitised: silently stripping the newline would
    // save something other than what was typed, and the operator would not be told.
    var attack = SSHConnection()
    attack.alias = "innocent"
    attack.hostName = "box.example.net\nHost *\n    User root"
    if case .success = SSHConfig.append(attack, configPath: path) {
        return fail("a hostname carrying a newline was ACCEPTED -- that appends a global "
            + "Host * block to the operator's config")
    }
    // Every field, not just the obvious one: the injection works from any of them.
    for (name, mutate) in [
        ("user", { (c: inout SSHConnection) in c.user = "me\nHost *" }),
        ("identityFile", { (c: inout SSHConnection) in c.identityFile = "k\nHost *" }),
        ("proxyJump", { (c: inout SSHConnection) in c.proxyJump = "j\nHost *" }),
        ("remoteCommand", { (c: inout SSHConnection) in c.remoteCommand = "ls\nHost *" }),
    ] {
        var probe = SSHConnection()
        probe.alias = "probe-\(name)"
        probe.hostName = "box.example.net"
        mutate(&probe)
        if case .success = SSHConfig.append(probe, configPath: path) {
            return fail("a newline in \(name) was accepted")
        }
    }

    // A duplicate alias must be refused. ssh takes the FIRST value, so a second block is
    // shadowed and looks like settings that did nothing.
    var duplicate = SSHConnection()
    duplicate.alias = "already-here"
    duplicate.hostName = "second.example.net"
    if case .success = SSHConfig.append(duplicate, configPath: path) {
        return fail("a duplicate alias was appended; ssh would shadow it silently")
    }

    // Nothing above should have touched the file at all.
    guard let afterRefusals = try? String(contentsOfFile: path, encoding: .utf8),
        afterRefusals == existing
    else { return fail("a REFUSED write still modified the file") }

    var good = SSHConnection()
    good.alias = "prod"
    good.hostName = "prod.example.net"
    good.user = "orel"
    good.port = "2222"
    good.identityFile = "~/.ssh/id_prod"
    good.proxyJump = "bastion"
    good.options = "ForwardAgent=yes\n\nServerAliveInterval=30\n"
    good.remoteCommand = "tmux attach"
    switch SSHConfig.append(good, configPath: path) {
    case .failure(let why):
        return fail("a valid connection was refused: \(why.summary)")
    case .success(let saved) where saved != "prod":
        return fail("saved under \(saved), expected prod")
    case .success:
        break
    }

    guard let written = try? String(contentsOfFile: path, encoding: .utf8) else {
        return fail("the config vanished")
    }
    // APPEND, not rewrite: every original byte still leads the file.
    if !written.hasPrefix(existing) {
        return fail("the existing config was rewritten rather than appended to")
    }
    // The unterminated last line must not have been joined onto the new block.
    if written.contains("User firstHost") || written.contains("first# added") {
        return fail("the new block was joined onto the previous unterminated line")
    }
    for expected in [
        "Host prod", "HostName prod.example.net", "User orel", "Port 2222",
        "IdentityFile ~/.ssh/id_prod", "ProxyJump bastion", "ForwardAgent yes",
        "ServerAliveInterval 30", "RemoteCommand tmux attach", "RequestTTY yes",
    ] where !written.contains(expected) {
        return fail("the written block is missing \(expected)")
    }

    // Round trip: the parser must see what the writer wrote, or the two halves of this
    // feature disagree and the saved host never appears in the sidebar.
    let hosts = SSHConfig.hosts(configPath: path)
    guard let prod = hosts.first(where: { $0.alias == "prod" }),
        prod.hostName == "prod.example.net", prod.user == "orel", prod.port == 2222
    else { return fail("the writer's own output did not survive the reader") }
    if hosts.count != 2 {
        return fail("expected 2 hosts after the append, got \(hosts.count)")
    }

    // A fresh file must be created 0600. Created world-readable, this surfaces much later
    // as ssh refusing a key, with an error that names the key and not the directory.
    let fresh = root + "/fresh/config"
    guard case .success = SSHConfig.append(good, configPath: fresh) else {
        return fail("could not create a config that did not exist")
    }
    let mode = (try? FileManager.default.attributesOfItem(atPath: fresh)[.posixPermissions])
    if (mode as? NSNumber)?.intValue != 0o600 {
        return fail("a freshly created config is mode \(mode ?? "unknown"), expected 0600")
    }

    // The dialled path, which shares nothing with the written path: argv, so a path with a
    // space in it stays one argument instead of being word-split by a shell.
    var spaced = SSHConnection()
    spaced.hostName = "box"
    spaced.user = "orel"
    spaced.identityFile = "/tmp/my key"
    spaced.port = "2200"
    if spaced.arguments != ["ssh", "-p", "2200", "-i", "/tmp/my key", "orel@box"] {
        return fail("argv was \(spaced.arguments)")
    }

    // THE SHELL INJECTION CASE, and it is the most dangerous assertion in this file.
    //
    // The host runs the command string through `/bin/sh -c`, so a host or alias carrying a
    // semicolon is TWO commands. No validator above rejects a semicolon and none should -
    // the fix belongs at the spawn boundary, where every word is quoted, and this is the
    // check that the fix is actually there. Single quotes make everything inside literal,
    // so the payload must survive as ONE word with its metacharacters inert.
    var evil = SSHConnection()
    evil.hostName = "box.example.net"
    evil.alias = "gone"
    let payloads = [
        "box; touch /tmp/mind2t-pwned", "box$(touch /tmp/mind2t-pwned)",
        "box`touch /tmp/mind2t-pwned`", "box | sh", "it's a box",
    ]
    for payload in payloads {
        let line = SSHConnection.shellQuoted(["ssh", payload])
        // Everything after `ssh ` must be inside one quoted run. The proof is that the
        // dangerous character is never outside quotes: count quote marks and require the
        // payload's metacharacters to sit between an odd and an even one.
        guard line.hasPrefix("'ssh' '") , line.hasSuffix("'") else {
            return fail("quoting produced \(line)")
        }
        for metacharacter in [";", "$", "`", "|"] where payload.contains(metacharacter) {
            // A quoted word cannot contain an unescaped quote, so splitting on `'\''`
            // boundaries is unnecessary: the only way a metacharacter escapes is if the
            // quoting was dropped entirely, which the prefix check above already denies.
            if line.contains("' \(metacharacter)") || line.contains("\(metacharacter) '") {
                return fail("\(metacharacter) escaped the quoting in \(line)")
            }
        }
    }
    // And the apostrophe case, which is the one quoting scheme's own failure mode.
    if SSHConnection.shellQuoted(["it's"]) != "'it'\\''s'" {
        return fail("an apostrophe broke the quoting: \(SSHConnection.shellQuoted(["it's"]))")
    }
    // Round trip through a real shell. Nothing else here proves the quoting works against
    // the thing that actually parses it, and `/bin/sh` is the oracle that can say NO.
    let probe = Process()
    probe.executableURL = URL(fileURLWithPath: "/bin/sh")
    probe.arguments = ["-c", "printf '%s\\n' " + SSHConnection.shellQuoted(payloads)]
    let pipe = Pipe()
    probe.standardOutput = pipe
    do {
        try probe.run()
        let out = String(
            decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        probe.waitUntilExit()
        let words = out.split(separator: "\n").map(String.init)
        if words != payloads {
            return fail("/bin/sh re-split the quoted words into \(words)")
        }
    } catch {
        return fail("could not run the shell oracle: \(error)")
    }
    if FileManager.default.fileExists(atPath: "/tmp/mind2t-pwned") {
        try? FileManager.default.removeItem(atPath: "/tmp/mind2t-pwned")
        return fail("a payload EXECUTED -- the quoting is not reaching the spawn boundary")
    }

    // The form's field mapping, driven through the real view. A gate that built its own
    // SSHConnection would pass while the Port field was wired to the User label.
    let form = SSHConnectForm()
    form.fill(good)
    let readBack = form.connection
    if readBack.hostName != good.hostName || readBack.user != good.user
        || readBack.port != good.port || readBack.identityFile != good.identityFile
        || readBack.proxyJump != good.proxyJump || readBack.alias != good.alias
        || readBack.remoteCommand != good.remoteCommand
        || readBack.optionList != good.optionList
    {
        return fail("the form did not read back what was written into it: \(readBack)")
    }

    print(
        "SSH WRITE SMOKE OK: shell injection quoted inert and confirmed by /bin/sh itself,"
            + " newline injection refused in 5 fields, duplicate alias"
            + " refused, refusals left the file byte-identical, the append preserved every"
            + " existing byte and did not join the unterminated last line, the writer's"
            + " output round-tripped through the reader, a fresh config is 0600, and a"
            + " spaced identity path stayed one argv word")
    return 0
}

/// `--smoke-ssh-layout`: the connection dialog's GEOMETRY, with no window shown.
///
/// This exists because the form shipped in v0.28.0 looking broken and nothing could tell.
/// Two controls in it had collapsed to slivers and the whole dialog stood 590pt tall over
/// an 816x510 window, and every gate was green, because the gates drive `Session` and stop
/// at the AppKit layer. A screenshot found it. A screenshot is not a gate.
///
/// The two failures it guards are the two that happened, and both are silent:
///
/// 1. **A control with no intrinsic width collapses inside `NSGridView`.** An `NSTextField`
///    is stretched by a fixed column width; an `NSScrollView` or an `NSStackView` is not,
///    because they report `noIntrinsicMetric` and the default placement is leading. The
///    result is a field that renders as a 30pt stub next to a correct-looking label, and
///    nothing in the layout is an error.
/// 2. **A dialog taller than the window it belongs to.** There is no clamp anywhere in
///    AppKit for this; the panel simply hangs off the window and reads as a leak.
///
/// It walks the real view tree rather than asking the form what it built, so a field
/// renamed, re-parented or dropped is visible to it. The COUNTS are the control: an
/// assertion about widths alone passes a dialog that lost half its fields.
func runSSHLayoutSmoke() -> Int32 {
    func fail(_ why: String) -> Int32 {
        FileHandle.standardError.write(Data("ssh layout smoke: \(why)\n".utf8))
        return 1
    }

    // AppKit refuses to lay out a view tree without an application object, and a
    // `.prohibited` policy keeps this off the Dock and out of the operator's way.
    let probeApp = NSApplication.shared
    probeApp.setActivationPolicy(.prohibited)

    func descendants(of view: NSView) -> [NSView] {
        view.subviews.flatMap { [$0] + descendants(of: $0) }
    }

    // The CONTAINER's tree, never the bare form. The form measured on its own reports
    // every field at 320pt in the exact build that shipped two collapsed controls, so an
    // oracle rooted at the form is a check placed before the thing it guards.
    let (dialogSize, root) = SSHConnect.measureDialog()
    let inside = descendants(of: root)

    // An editable NSTextField is an input; the labels in column 0 are not, and neither is
    // the placeholder cell AppKit parks inside a scroll view.
    let inputs = inside.compactMap { $0 as? NSTextField }.filter { $0.isEditable }
    let scrollers = inside.compactMap { $0 as? NSScrollView }
    let buttons = inside.compactMap { $0 as? NSButton }

    guard inputs.count == 7 else {
        return fail("expected 7 editable fields, found \(inputs.count) -- the width"
            + " assertions below are vacuous on a form that lost fields")
    }
    guard scrollers.count == 1 else {
        return fail("expected 1 scroll view for Options, found \(scrollers.count)")
    }
    // Found by title rather than by count, because the container contributes its own
    // action buttons to this tree. Their presence is asserted too: a dialog measured
    // without its buttons is measured too short, which is the direction that hides a leak.
    guard let choose = buttons.first(where: { $0.title.hasPrefix("Choose") }) else {
        return fail("the Choose... button is not in the dialog's view tree")
    }
    for title in ["Save & Connect", "Connect", "Cancel"] where
        !buttons.contains(where: { $0.title == title })
    {
        return fail("the dialog has no \(title) button -- its height is being measured"
            + " without the button row")
    }
    // Exactly one Return and exactly one Escape. Zero means a dialog the keyboard cannot
    // finish; two means Return fires whichever button AppKit reaches first, which is a
    // coin flip between saving to the operator's ssh config and not.
    let byReturn = buttons.filter { $0.keyEquivalent == "\r" }
    let byEscape = buttons.filter { $0.keyEquivalent == "\u{1b}" }
    guard byReturn.count == 1, byReturn[0].title == "Save & Connect" else {
        return fail("Return maps to \(byReturn.map(\.title)), expected exactly"
            + " [Save & Connect]")
    }
    guard byEscape.count == 1, byEscape[0].title == "Cancel" else {
        return fail("Escape maps to \(byEscape.map(\.title)), expected exactly [Cancel]")
    }
    // Asserted as a property, not read off a screenshot. macOS dims accent colour in a
    // window that is not key, and the capture mode's window never is (making it key would
    // mean activating the app over whatever the operator is using). So the picture cannot
    // answer "is the primary action marked" and this can.
    guard byReturn[0].bezelColor != nil else {
        return fail("the primary action carries no bezel tint -- all three buttons render"
            + " identically and nothing marks which one Return performs")
    }

    // Every check below runs, and they are reported together. A gate that returns on the
    // first geometry failure costs one full build per defect, and these arrive in groups:
    // the two collapsed controls here had one cause and would have taken two rounds.
    var faults: [String] = []

    // The measurements themselves, always, pass or fail. A geometry gate that prints only
    // a verdict makes the next person re-instrument it to learn anything, and the numbers
    // are the part worth reading.
    for (index, field) in inputs.enumerated() {
        print(String(
            format: "  field[%d] %4dx%-3d", index,
            Int(field.frame.width), Int(field.frame.height)))
    }
    print(String(
        format: "  options  %4dx%-3d   choose %4dx%-3d",
        Int(scrollers[0].frame.width), Int(scrollers[0].frame.height),
        Int(choose.frame.width), Int(choose.frame.height)))
    print(String(
        format: "  dialog   %4dx%-3d", Int(dialogSize.width), Int(dialogSize.height)))

    // 150pt is not a taste threshold. It is well below any laid-out field in a 320pt
    // column and well above the ~30pt stub a collapsed control produces, so it separates
    // the two states and says nothing about the ones in between.
    let floor: CGFloat = 150
    for field in inputs where field.frame.width < floor {
        faults.append("an editable field is \(Int(field.frame.width))pt wide, under"
            + " \(Int(floor))pt -- a control with no intrinsic width collapsed in the grid")
    }
    if scrollers[0].frame.width < floor {
        faults.append("the Options field is \(Int(scrollers[0].frame.width))pt wide, under"
            + " \(Int(floor))pt -- NSScrollView reports no intrinsic width and the grid"
            + " left it at its stub size")
    }
    if scrollers[0].frame.height < 40 {
        faults.append("the Options field is \(Int(scrollers[0].frame.height))pt tall")
    }
    if choose.frame.width < 60 {
        faults.append("the Choose... button is \(Int(choose.frame.width))pt wide")
    }

    // The leak. 460 is derived, not chosen: the smallest window this host will open is
    // 510pt tall (the 120pt pane floor plus chrome at the sizes `--smoke-chrome` covers),
    // and a dialog must sit inside its own window with room for the title bar.
    let ceiling: CGFloat = 460
    let size = dialogSize
    if size.height > ceiling {
        faults.append("the dialog stands \(Int(size.height))pt tall, over the"
            + " \(Int(ceiling))pt ceiling -- it hangs off the bottom of the window it"
            + " belongs to")
    }
    if size.width > 620 {
        faults.append("the dialog is \(Int(size.width))pt wide, over 620pt")
    }

    guard faults.isEmpty else {
        return fail("\(faults.count) fault(s)\n  - " + faults.joined(separator: "\n  - "))
    }

    print(
        "SSH LAYOUT SMOKE OK: 7 fields, the Options box and the Choose button all laid out"
            + " above the collapse floor, dialog \(Int(size.width))x\(Int(size.height))pt"
            + " inside the \(Int(ceiling))pt ceiling")
    return 0
}

/// `--shot-ssh-dialog <path>`: renders the connection dialog to a PNG and exits.
///
/// Not a gate. It is the reason the gate above exists at all: the v0.28.0 form was proven
/// by five green gates and was visibly broken, and the only thing that found it was
/// looking at a screenshot of the operator's screen. This makes that loop available
/// without a window, without his screen, and without raising anything.
///
/// `--smoke-ssh-layout` answers "is any control collapsed"; this answers "does it look
/// right", which is a different question and not one an assertion can hold.
func runSSHDialogShot(_ path: String, dark: Bool) -> Int32 {
    let probeApp = NSApplication.shared
    // `.accessory`, never `.prohibited`: a prohibited app's windows are not composited, so
    // the capture below comes back empty or partial. No Dock icon either way.
    probeApp.setActivationPolicy(.accessory)
    probeApp.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)

    let dialog = SSHConnectDialog()
    dialog.layoutSubtreeIfNeeded()

    // A REAL window, ordered front, parked far outside any display. `cacheDisplay` and
    // `dataWithPDF` both silently drop layer-backed controls: the first attempt at this
    // produced an image with the seven text fields and NOTHING else - no labels, no
    // buttons, no title - which is a picture that would have sent the next session
    // hunting a layout bug that does not exist. The window server's own composite is the
    // only capture that shows what the operator would see.
    let host = SSHConnect.makeSheet(dialog)
    host.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
    host.setFrameOrigin(NSPoint(x: -8000, y: -8000))
    host.orderFrontRegardless()
    host.displayIfNeeded()
    // The window server needs a turn of the loop before the surface holds anything.
    RunLoop.current.run(until: Date().addingTimeInterval(0.4))

    guard let shot = CGWindowListCreateImage(
        .null, .optionIncludingWindow, CGWindowID(host.windowNumber),
        [.boundsIgnoreFraming, .bestResolution]), shot.width > 1
    else {
        FileHandle.standardError.write(Data("the window server returned no image\n".utf8))
        return 1
    }
    let rep = NSBitmapImageRep(cgImage: shot)
    guard let png = rep.representation(using: .png, properties: [:]) else {
        FileHandle.standardError.write(Data("could not encode a PNG\n".utf8))
        return 1
    }
    do {
        try png.write(to: URL(fileURLWithPath: path))
    } catch {
        FileHandle.standardError.write(Data("could not write \(path): \(error)\n".utf8))
        return 1
    }
    print("wrote \(path) at \(rep.pixelsWide)x\(rep.pixelsHigh)")
    return 0
}

let arguments = CommandLine.arguments
if arguments.contains("--smoke") {
    exit(runSmoke())
}
if arguments.contains("--smoke-ssh-layout") {
    exit(runSSHLayoutSmoke())
}
if let index = arguments.firstIndex(of: "--shot-ssh-dialog"), index + 1 < arguments.count {
    exit(runSSHDialogShot(arguments[index + 1], dark: !arguments.contains("--light")))
}
if arguments.contains("--smoke-ssh-write") {
    exit(runSSHWriteSmoke())
}
if arguments.contains("--smoke-chrome") {
    exit(runChromeSmoke())
}
if arguments.contains("--smoke-ssh") {
    exit(runSSHSmoke())
}
if arguments.contains("--smoke-agent") {
    exit(runAgentSmoke())
}
if arguments.contains("--smoke-worktree") {
    exit(runWorktreeSmoke())
}
if arguments.contains("--smoke-history") {
    exit(runHistorySmoke())
}

var webDirArgument: String?
if let index = arguments.firstIndex(of: "--web-dir"), index + 1 < arguments.count {
    webDirArgument = arguments[index + 1]
}
if arguments.contains("--smoke-panel") || arguments.contains("--smoke-panel-control") {
    // A WKWebView needs an application object to run its document.
    let probeApp = NSApplication.shared
    probeApp.setActivationPolicy(.prohibited)
    exit(
        runPanelSmoke(
            webDir: webDirArgument, control: arguments.contains("--smoke-panel-control")))
}

var command: String?
if let index = arguments.firstIndex(of: "--command"), index + 1 < arguments.count {
    command = arguments[index + 1]
}

// Settings (S1): ~/.ruuah/config.toml, or --config-dir for a capture/test that must not
// touch the real one. Loading never fails into an unusable state; anything that could
// not be honoured arrives as an error string the app shows loudly on launch.
var configDir: String?
if let index = arguments.firstIndex(of: "--config-dir"), index + 1 < arguments.count {
    configDir = arguments[index + 1]
}
var config: OpaquePointer?
if let configDir {
    _ = configDir.withCString { pointer in mind2t_config_load(pointer, &config) }
} else {
    _ = mind2t_config_load(nil, &config)
}
let configError = mind2t_config_error(config).map { String(cString: $0) }

// When the binary runs from an assembled .app (scripts/build-app.sh), the app is
// Hebrew-first: auto base direction unless --ltr. The bare CLI binary keeps the
// flag-driven defaults unchanged. A window opens straight into the login shell --
// no splash, like Ghostty; the bundled banner remains available via --splash.
let bundledBanner = Bundle.main.path(forResource: "banner", ofType: "sh")
if command == nil, arguments.contains("--splash"), let bundledBanner {
    command = "sh '\(bundledBanner)'"
}
// The config's shell is the default for new sessions; explicit CLI intent outranks it.
if command == nil, !arguments.contains("--splash"),
    let shell = mind2t_config_shell(config).map({ String(cString: $0) })
{
    command = shell
}
// CLI flags outrank the config, which outranks the bundle default (the .app is
// Hebrew-first; the bare CLI binary defaults LTR).
let autoDirection: Bool
if arguments.contains("--auto-direction") {
    autoDirection = true
} else if arguments.contains("--ltr") {
    autoDirection = false
} else {
    autoDirection = mind2t_config_auto_direction(config, bundledBanner != nil)
}
// Logical size; the delegate multiplies by the backing scale at spawn.
let configFontSize = mind2t_config_font_size(config)
let baseFontSize: Float = configFontSize > 0 ? configFontSize : 16

// Shell integration (S2): spawned shells inherit this process's environment, so pointing
// ZDOTDIR at the bundled bootstrap is all the wiring blocks need. zsh-only by nature (the
// variable means nothing to other shells), .app-only in practice (the bare CLI binary has
// no resource bundle, and an externally-set ZDOTDIR chain is left alone there).
if let resources = Bundle.main.resourcePath {
    let zdotdir = resources + "/shell/zdotdir"
    let integration = resources + "/shell/mind2t-integration.zsh"
    if FileManager.default.fileExists(atPath: zdotdir + "/.zshenv"),
        FileManager.default.fileExists(atPath: integration)
    {
        setenv("MIND2T_INTEGRATION", integration, 1)
        if let original = getenv("ZDOTDIR") {
            setenv("MIND2T_USER_ZDOTDIR", String(cString: original), 1)
        }
        setenv("ZDOTDIR", zdotdir, 1)
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let delegate = HostAppDelegate(
    command: command, autoDirection: autoDirection, config: config,
    baseFontSize: baseFontSize, configError: configError, configDir: configDir,
    webDir: webDirArgument)
app.delegate = delegate
app.run()
