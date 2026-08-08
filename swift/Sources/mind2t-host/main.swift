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
private func withOptionalCString<R>(
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

/// The S5.5 dock geometry: the pane and the sidebar must TILE the content exactly.
///
/// This is the silent one. A pane that keeps its full width while a sidebar is docked
/// draws underneath it: the terminal looks perfectly normal and its right-hand columns
/// are simply covered, which no screenshot of the sidebar would reveal. Overlap and gap
/// are both failures, and both are invisible to "does it look right".
func runDockSmoke() -> Int32 {
    let tab: CGFloat = 38
    let width: CGFloat = 300

    func fail(_ message: String) -> Int32 {
        FileHandle.standardError.write(Data("DOCK SMOKE FAILED: \(message)\n".utf8))
        return 1
    }

    // Undocked: the pane owns the whole width and there is no sidebar rect at all.
    let open = ChromeLayout.compute(
        content: NSSize(width: 1120, height: 700), tabHeight: tab, sidebarWidth: nil)
    guard open.sidebar == nil, open.pane.width == 1120 else {
        return fail("undocked pane must own the full width, got \(open.pane.width)")
    }

    // Docked: exact tiling, and the pane genuinely gave up the width.
    let docked = ChromeLayout.compute(
        content: NSSize(width: 1120, height: 700), tabHeight: tab, sidebarWidth: width)
    guard let sidebar = docked.sidebar else { return fail("docked layout has no sidebar rect") }
    guard docked.pane.width == 1120 - width else {
        return fail("docked pane should be \(1120 - width), got \(docked.pane.width)")
    }
    guard sidebar.minX == docked.pane.maxX else {
        return fail(
            "pane ends at \(docked.pane.maxX) and the sidebar starts at \(sidebar.minX): "
                + (sidebar.minX < docked.pane.maxX ? "they OVERLAP" : "there is a GAP"))
    }
    guard docked.pane.width + sidebar.width == 1120 else {
        return fail("pane + sidebar = \(docked.pane.width + sidebar.width), content is 1120")
    }
    guard docked.pane.height == sidebar.height, docked.pane.height == 700 - tab else {
        return fail("pane and sidebar must share the band below the tab bar")
    }

    // A window narrower than the sidebar's preferred width: the pane's floor wins and
    // the sidebar takes the remainder. Computing the sidebar as a constant instead of a
    // remainder puts them back on top of each other exactly here.
    let narrow = ChromeLayout.compute(
        content: NSSize(width: 200, height: 400), tabHeight: tab, sidebarWidth: width)
    guard let narrowSidebar = narrow.sidebar else { return fail("narrow layout has no sidebar") }
    guard narrow.pane.width == ChromeLayout.minimumPaneWidth else {
        return fail("the pane floor must hold, got \(narrow.pane.width)")
    }
    guard narrow.pane.width + narrowSidebar.width == 200, narrowSidebar.width >= 0 else {
        return fail(
            "narrow window does not tile: pane \(narrow.pane.width) + sidebar "
                + "\(narrowSidebar.width) != 200")
    }

    print(
        "DOCK SMOKE OK: 1120 tiles as \(docked.pane.width) + \(sidebar.width); "
            + "at 200 the pane floor holds and the sidebar takes \(narrowSidebar.width)")
    return 0
}

let arguments = CommandLine.arguments
if arguments.contains("--smoke") {
    exit(runSmoke())
}
if arguments.contains("--smoke-worktree") {
    exit(runWorktreeSmoke())
}
if arguments.contains("--smoke-dock") {
    exit(runDockSmoke())
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
