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

let arguments = CommandLine.arguments
if arguments.contains("--smoke") {
    exit(runSmoke())
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
