// The app: a top tab strip over one terminal surface.
//
// Each session is its own Mind2tHost -- pty, pump thread, renderer -- and the app blits
// whichever is active. Background sessions keep running and are polled at a low rate
// only to notice exits; their frames queue in the seqlock (the writer never blocks), so
// a switch shows the newest state immediately.

import AppKit
import CMind2tHost
import UserNotifications

final class HostAppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate,
    TabBarDelegate, SidebarDelegate
{
    private let command: String?
    private let autoDirection: Bool
    /// The loaded settings handle; contributes the theme palette at every spawn. Owned
    /// for the app's lifetime (the borrowed strings read at launch depend on it).
    private let config: OpaquePointer?
    /// Logical font size from the config (or the default); scaled per spawn.
    private let baseFontSize: Float
    /// The configured lead font family, for metric queries that must match the
    /// renderer's own stack (zoom math with the wrong font drifts the grid).
    private let fontFamily: String?
    /// What could not be honoured in the config, shown once at launch -- a settings file
    /// that silently half-applies looks like a broken app, so the failure is loud.
    private let configError: String?
    /// Overrides the workflows directory the same way it overrides config.toml, so
    /// captures and taps never touch (or require) the real ~/.ruuah.
    private let configDir: String?
    /// Where the built panel document lives, when it is not in the app bundle. The bare
    /// CLI binary has no resource bundle, so the smoke and any tap pass `--web-dir`.
    private let webDir: String?
    private var window: NSWindow!
    private var view: TerminalView!

    /// B1: present frames on the GPU instead of copying them back and drawing a CGImage.
    ///
    /// An environment switch rather than a config key, on purpose. This is a temporary A/B
    /// against the path it replaces, not a preference anyone should carry: once presenting is
    /// proven it becomes the only path and this disappears, whereas a config key would have to
    /// be parsed, defaulted, documented and then deprecated. `MIND2T_GPU_PRESENT=1`.
    private let gpuPresent = ProcessInfo.processInfo.environment["MIND2T_GPU_PRESENT"] == "1"
    /// The session whose host currently owns the metal layer. Only one can: the layer is a
    /// single swapchain, and attaching a second host to it would have two renderers presenting
    /// into the same drawable.
    private var presentingSession: Session?
    private var tabBar: TabBarView!
    /// The docked workspace list. Docked unless `MIND2T_NO_SIDEBAR=1` -- an env switch
    /// rather than a config key for the same reason `gpuPresent` is one: it exists so the
    /// smoke gate can drive BOTH geometries and compare them, and a config key would have
    /// to be parsed, defaulted, documented and then deprecated.
    private var sidebar: SidebarView!
    private let sidebarDocked = ProcessInfo.processInfo.environment["MIND2T_NO_SIDEBAR"] != "1"
    /// Branch per directory, computed on miss and never invalidated within a run.
    ///
    /// `git rev-parse` costs a process, and `refreshChrome` is driven by title and progress
    /// events that a busy agent emits several times a second. Uncached, this would spawn git
    /// at that rate on the main thread. The stated cost of never invalidating: a branch
    /// switched underneath a running session shows stale until the session moves directory.
    private var branchCache: [String: String] = [:]
    private var timer: Timer?

    private var sessions: [Session] = []
    private var activeIndex = -1
    private var spawnCount = 0
    private var windowSized = false
    private var tick: UInt64 = 0
    /// Zoom multiplier over the configured base size (cmd+= / cmd+- / cmd+0).
    private var fontScale: Float = 1.0
    /// Background sessions are polled once per this many active ticks (~2 Hz at 60).
    private static let backgroundEvery: UInt64 = 30

    // MARK: web panels (S6)

    /// The open panel, or nil. At most one: panels are modal-feeling overlays, and two
    /// of them competing for Escape is a worse problem than any layout it would buy.
    private var webPanel: WebPanel?
    /// The repository the open panel is showing, and the file list it was given. The
    /// list is the ALLOWED SET for `requestDiff`: a path the host never advertised is
    /// refused rather than handed to git, so the panel cannot widen its own reach.
    private var panelRoot: String?
    private var panelFiles: [ChangedFile] = []
    /// git runs here, never on the main thread -- `status` on a large tree is tens of
    /// milliseconds and the terminal is blitting at 60 Hz on the other side of it.
    private let gitQueue = DispatchQueue(label: "mind2t.git", qos: .userInitiated)

    init(
        command: String?, autoDirection: Bool, config: OpaquePointer?,
        baseFontSize: Float, configError: String?, configDir: String? = nil,
        webDir: String? = nil
    ) {
        self.command = command
        self.autoDirection = autoDirection
        self.config = config
        self.baseFontSize = baseFontSize
        self.configError = configError
        self.configDir = configDir
        self.webDir = webDir
        if let config, let family = mind2t_config_font_family(config) {
            self.fontFamily = String(cString: family)
        } else {
            self.fontFamily = nil
        }
    }

    /// Puts one polled frame on screen, whichever path is live.
    ///
    /// While presenting there IS no image - the host leaves `pixels` null so the frame never
    /// crosses the bus - so the argument is nil every time and the work is a present call.
    private func showFrame(_ image: CGImage?) {
        if let session = activeSession, session.presenting {
            let ok = session.present()
            if !ok, !presentReported {
                presentReported = true
                FileHandle.standardError.write(Data("MIND2T_PRESENT=failed\n".utf8))
            }
            // The CGImage path is NOT run as a fallback here on purpose. Drawing the old way
            // when a present fails would hide the failure behind a working-looking window,
            // which is the whole class of bug this instrumentation exists to expose.
            return
        }
        if let image {
            view.contentLayer.contents = image
        }
    }

    /// One report per run. A present that fails does so every frame, and 60 lines a second
    /// buries the message it is trying to deliver.
    private var presentReported = false

    /// Moves the metal layer to whichever session is active, when presenting is switched on.
    ///
    /// One layer means one swapchain, so exactly one host may own it; attaching a second would
    /// have two renderers writing the same drawable. The previous owner is detached first,
    /// which also restores its readback path, so a failed attach leaves a working session
    /// rather than a blank one.
    private func syncPresentTarget(to session: Session?) {
        guard gpuPresent, presentingSession !== session else { return }
        presentingSession?.detachLayer()
        presentingSession = nil
        // The outgoing session's last image would otherwise sit on top of the metal layer
        // forever, which looks exactly like a frozen terminal.
        view.contentLayer.contents = nil
        guard let session else { return }
        // Size the layer BEFORE attaching. Activation can run before any layout pass, and an
        // unsized CAMetalLayer reports a zero drawable which the swapchain clamps to 1x1 - an
        // attach that succeeds and then shows nothing, while the stale CGImage underneath
        // makes the window look correct. Measured 2026-08-04: the first live run reported
        // `attach=ok drawable=1x1` and the picture on screen was the old path's.
        view.layoutSubtreeIfNeeded()
        let size = view.presentLayer.drawableSize
        let ok = session.attachLayer(
            view.presentLayer, width: Int(size.width), height: Int(size.height))
        // Reported on FAILURE only. A failed attach falls back to the readback path and the
        // window looks EXACTLY the same, so silence there would let "it drew something" pass
        // as proof the GPU path ran. Success needs no announcement; the zero-readback test
        // covers that side.
        if !ok {
            let report = "MIND2T_PRESENT_ATTACH=failed "
                + "drawable=\(Int(size.width))x\(Int(size.height))\n"
            FileHandle.standardError.write(Data(report.utf8))
        }
        if ok {
            presentingSession = session
            // Push the size again now that an owner exists. `onPresentResize` fires from
            // layout, and a layout that ran while nothing was attached delivered its size to
            // nobody - so ordering alone decided whether the swapchain was ever correct.
            session.resizeLayer(width: Int(size.width), height: Int(size.height))
        }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        view = TerminalView(frame: .zero)
        view.wantsLayer = true
        view.onPresentResize = { [weak self] width, height in
            self?.presentingSession?.resizeLayer(width: Int(width), height: Int(height))
        }

        tabBar = TabBarView(frame: .zero)
        tabBar.delegate = self
        sidebar = SidebarView(frame: .zero)
        sidebar.delegate = self

        // The bar owns the title-bar band: transparent titlebar + full-size content,
        // traffic lights inline at its left -- the reference's chrome.
        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1120, height: 700),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title =
            (Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String)
            ?? "mind2t-vt host"
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden

        let content = NSView(frame: window.contentLayoutRect)
        content.autoresizesSubviews = true
        content.addSubview(tabBar)
        content.addSubview(view)
        // Added AFTER the terminal view deliberately. The pane and the sidebar tile
        // exactly, so ordering is not load-bearing for correctness -- but if the tiling
        // arithmetic is ever wrong, a sidebar added last draws ON TOP and the defect is
        // visible, whereas added first it hides under the pane and looks like a sidebar
        // that failed to appear. Fail loudly.
        if sidebarDocked { content.addSubview(sidebar) }
        window.contentView = content
        layoutChrome()
        window.delegate = self
        window.center()
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(view)
        NSApp.activate(ignoringOtherApps: true)

        // Native resolution: the renderer rasterizes at backing scale, the layer declares
        // it, and one buffer pixel is one device pixel.
        view.layer?.backgroundColor = NSColor.black.cgColor
        // Below the content layer: the metal layer carries the frame, the content layer keeps
        // its sublayers (the ghost suggestion) and its geometry, and overlays stay on top.
        // Device and pixel format are deliberately NOT set here - the surface owns them, and a
        // layer configured with one device while the renderer draws on another is a black
        // window with no error anywhere.
        view.presentLayer.isOpaque = true
        view.presentLayer.contentsScale = window.backingScaleFactor
        view.layer?.addSublayer(view.presentLayer)
        view.layer?.addSublayer(view.contentLayer)
        view.contentLayer.contentsScale = window.backingScaleFactor
        view.contentLayer.magnificationFilter = .nearest
        // A frame is NEVER scaled to fit its layer. The default gravity is `.resize`,
        // which stretches whatever was last drawn across the new bounds -- so during a
        // resize, before the pty has caught up, the terminal renders as soft, oversized
        // glyphs with the previous frame's edges smeared over the gap. Anchoring at the
        // top-left leaves a stale frame at its true pixel size with the background
        // showing past it, which is what every terminal does mid-drag and reads as
        // "not repainted yet" instead of "broken". (Operator-reported, 2026-08-02.)
        view.contentLayer.contentsGravity = .topLeft

        // Standalone CALayers implicitly ANIMATE property changes -- a 0.25s crossfade on
        // every `contents` swap. At a 60 Hz blit the overlapping fades read as typing at
        // five frames a second (found live, 2026-07-29). A terminal frame is a hard cut.
        let hardCut: [String: CAAction] = [
            "contents": NSNull(), "backgroundColor": NSNull(),
            "bounds": NSNull(), "position": NSNull(),
        ]
        view.contentLayer.actions = hardCut
        view.layer?.actions = hardCut

        // Typing and pasting snap the view to the live bottom first -- the standard
        // terminal policy, applied here because the C surface deliberately leaves it
        // to the embedder. A no-op when already at the bottom.
        view.onKey = { [weak self] action, key, mods, consumed, text, unshifted in
            guard let session = self?.activeSession else { return false }
            // Typing snaps to the live bottom, the policy the C surface leaves to the
            // embedder -- presses only, so a held modifier's release doesn't snap.
            if action != 0 {
                session.scroll(Int32.min)
            }
            return session.key(
                action: action, key: key, mods: mods, consumedMods: consumed,
                text: text, unshiftedCodepoint: unshifted)
        }
        view.onPaste = { [weak self] bytes in
            guard let session = self?.activeSession else { return }
            session.scroll(Int32.min)
            session.paste(bytes)
        }
        view.onScroll = { [weak self] rows in self?.activeSession?.scroll(rows) }
        view.onAcceptSuggestion = { [weak self] in
            guard let self, !self.ghostRemainder.isEmpty,
                let session = self.activeSession
            else { return false }
            // The paste path types the remainder; bracketed mode is its concern.
            session.paste(self.ghostRemainder)
            self.hideGhost()
            return true
        }
        view.onMouse = { [weak self] action, button, mods, x, y in
            self?.activeSession?.mouse(action: action, button: button, mods: mods, x: x, y: y)
                ?? false
        }
        view.onWheel = { [weak self] x, y, ticks, mods in
            self?.activeSession?.wheel(x: x, y: y, ticks: ticks, mods: mods) ?? false
        }
        // Geometry goes to every session: it is per-host state, and a background
        // session must not come to the front with a stale (or never-set) view size.
        view.onMouseGeometry = { [weak self] width, height, inset in
            self?.sessions.forEach { $0.mouseGeometry(width: width, height: height, inset: inset) }
        }
        view.onNewSession = { [weak self] in self?.newSession() }
        view.onPalette = { [weak self] in self?.togglePalette() }
        // Left nil when panels are off, so the chord falls through to the child rather
        // than being swallowed by a feature that is not there.
        if mind2t_config_panels(config) {
            view.onDiffPanel = { [weak self] in self?.toggleDiffPanel() }
            // The live-tap hook (SCAR-014): a panel is a GUI seam, and proving it means
            // seeing it in a real window. Opening it from the environment is what lets
            // that be a scripted capture instead of synthesized keystrokes into
            // whatever the operator happens to be doing.
            // "1"/"diff" opens the review card.
            let open = ProcessInfo.processInfo.environment["MIND2T_PANEL_OPEN"]
            if open == "1" || open == "diff" {
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in
                    self?.toggleDiffPanel()
                }
            }
        }
        // The S5 live tap, same reasoning: drives createWorkspace directly so a capture
        // exercises the real worktree + cwd + pill path without synthesizing a modal.
        if let branch = ProcessInfo.processInfo.environment["MIND2T_WORKSPACE_TAP"],
            !branch.isEmpty
        {
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) { [weak self] in
                guard let self, let directory = self.activeDirectory(),
                    let root = Git.repositoryRoot(containing: directory)
                else { return }
                self.createWorkspace(root: root, branch: branch)
            }
        }
        view.onCloseSession = { [weak self] in
            guard let self, self.activeIndex >= 0 else { return }
            self.closeSession(index: self.activeIndex)
        }
        view.onBlockClick = { [weak self] block, event in
            self?.showBlockMenu(block, with: event)
        }
        view.onCommandClick = { [weak self] point in self?.openLink(at: point) }
        view.onZoomIn = { [weak self] in self?.zoom(by: 1.1) }
        view.onZoomOut = { [weak self] in self?.zoom(by: 1 / 1.1) }
        view.onZoomReset = { [weak self] in self?.zoom(to: 1.0) }

        setupSuggestions()
        newSession()
        guard !sessions.isEmpty else {
            NSApp.terminate(nil)
            return
        }

        // For screenshots: `screencapture -l` takes this id. Written unbuffered, because a
        // scripted capture reads it while the app is still running.
        FileHandle.standardOutput.write(
            Data("MIND2T_HOST_WINDOW=\(window.windowNumber)\n".utf8))
        // The frame too, top-left origin, so scripted captures and synthetic-input
        // tests (SCAR-014 live taps) can aim without a window-server query.
        let screenHeight = window.screen?.frame.height ?? 0
        let frame = window.frame
        FileHandle.standardOutput.write(
            Data(
                "MIND2T_HOST_FRAME=\(Int(frame.origin.x)),\(Int(screenHeight - frame.origin.y - frame.height)),\(Int(frame.width)),\(Int(frame.height))\n"
                    .utf8))

        if let configError {
            FileHandle.standardError.write(Data("config: \(configError)\n".utf8))
            let alert = NSAlert()
            alert.alertStyle = .warning
            alert.messageText = "Settings could not be fully applied"
            alert.informativeText = "\(configError)\n\nDefaults are in effect for the parts that failed."
            alert.runModal()
        }

        timer = Timer.scheduledTimer(withTimeInterval: 1.0 / 60.0, repeats: true) {
            [weak self] _ in
            self?.pollTick()
        }
    }

    /// The bar band on top, the terminal pane below it -- recomputed on every window
    /// resize (manual frames; two views do not earn Auto Layout here).
    private func layoutChrome() {
        guard let content = window.contentView else { return }
        let layout = ChromeLayout.compute(
            content: content.bounds.size, tabHeight: TabBarView.height,
            sidebarWidth: sidebarDocked ? SidebarView.preferredWidth : nil)
        tabBar.frame = layout.tabBar
        tabBar.autoresizingMask = [.width, .minYMargin]
        view.frame = layout.pane
        // Manual frame for the same reason the pane takes one: the sidebar's width is a
        // REMAINDER of the pane's floor, which an autoresizing mask cannot express.
        if let rect = layout.sidebar {
            sidebar.frame = rect
            sidebar.autoresizingMask = []
            sidebar.needsLayout = true
        }
        // Manual frames, still: `gridForPane` derives cols and rows from `view.bounds`, so the
        // frame set here is what resizes the pty. An autoresizing mask would let AppKit move it
        // behind that derivation's back.
        view.autoresizingMask = []
    }

    private var activeSession: Session? {
        activeIndex >= 0 && activeIndex < sessions.count ? sessions[activeIndex] : nil
    }

    // MARK: session lifecycle

    private func newSession() {
        spawnCount += 1
        let word = command?.split(separator: " ").first.map(String.init) ?? "zsh"
        // The bundled splash command is an implementation detail nobody wants as a label.
        let title = word == "sh" ? "session \(spawnCount)" : "\(word) \(spawnCount)"
        let (cols, rows) = gridForPane()
        let scale = Float(window.backingScaleFactor)
        guard
            let session = Session(
                command: command, cols: cols, rows: rows,
                fontSize: baseFontSize * scale * fontScale,
                autoDirection: autoDirection, config: config, title: title)
        else { return }
        sessions.append(session)
        activate(index: sessions.count - 1)
    }

    // MARK: agents

    /// Opens a tab running `agent`, in the active session's directory.
    ///
    /// The directory is the point. "Launch Claude" nearly always means "launch it HERE", and
    /// an agent that opens at HOME while the operator is three levels into a repository has to
    /// be told where it is before it can do anything. `activeDirectory` is the live OSC 7
    /// report, so it follows the shell rather than where the pane started; nil (no shell
    /// integration, or nothing reported yet) falls back to HOME, which is the archive's own
    /// default and a defensible place to be.
    private func newAgentSession(_ agent: Agent) {
        spawnCount += 1
        let (cols, rows) = gridForPane()
        let scale = Float(window.backingScaleFactor)
        let outcome = Session.agent(
            agent, cols: cols, rows: rows, fontSize: baseFontSize * scale * fontScale,
            autoDirection: autoDirection, config: config, cwd: activeDirectory())
        switch outcome {
        case .success(let session):
            report("agent \(agent.id) at \(agent.path ?? "?") in \(activeDirectory() ?? "~")")
            sessions.append(session)
            activate(index: sessions.count - 1)
        case .failure(let why):
            // Named, never silent. A launch that fails without saying why is the shape that
            // sends an operator hunting through their own PATH for our bug.
            report("agent \(agent.id) did not launch: \(why.summary)")
            warn("Could not launch \(agent.name)", why.summary)
        }
    }

    /// One palette row per agent: installed ones launch, the rest hand over their install
    /// command.
    ///
    /// The uninstalled ones are LISTED rather than hidden, and they are not dead rows -- they
    /// copy the install line. Hiding them makes the feature invisible to anyone who has not
    /// already installed an agent, which is everyone the first time.
    private func agentPaletteItems() -> [PaletteItem] {
        let agents = Agent.all()
        let here = activeDirectory().map { ($0 as NSString).lastPathComponent }
        return agents.sorted { $0.isInstalled && !$1.isInstalled }.map { agent in
            if agent.isInstalled {
                return PaletteItem(
                    title: "Agent: \(agent.name)",
                    subtitle: here.map { "a new tab in \($0)" } ?? "a new tab",
                    workflowIndex: nil,
                    action: { [weak self] in self?.newAgentSession(agent) })
            }
            return PaletteItem(
                title: "Agent: \(agent.name)", subtitle: "not installed \u{2022} \(agent.installHint)",
                workflowIndex: nil,
                action: { [weak self] in self?.clip(agent.installHint) })
        }
    }

    // MARK: the sidebar's plus

    /// The `+` menu. Everything that opens a pane, in one visible place.
    ///
    /// A MENU rather than straight to the form, because four of the five things this
    /// offers need no form at all and going through one to reach them would be worse than
    /// the palette it is replacing. The saved hosts are listed here for the same reason
    /// the `+` exists: a feature reachable only from a fuzzy search box is invisible to
    /// anyone who has not been told it is there.
    func sidebarDidRequestNew(from view: NSView) {
        let menu = NSMenu()
        menu.addItem(withTitle: "New Session", action: #selector(menuNewSession), keyEquivalent: "")
        menu.addItem(
            withTitle: "New Workspace...", action: #selector(menuNewWorkspace), keyEquivalent: "")
        menu.addItem(.separator())

        let hosts = SSHConfig.hosts()
        if hosts.isEmpty {
            let empty = NSMenuItem(title: "No hosts in ~/.ssh/config", action: nil, keyEquivalent: "")
            empty.isEnabled = false
            menu.addItem(empty)
        }
        for host in hosts.prefix(HostAppDelegate.sshMenuLimit) {
            let item = NSMenuItem(
                title: host.alias, action: #selector(menuSSHHost(_:)), keyEquivalent: "")
            item.target = self
            // The subtitle the palette shows, as a tooltip: a menu row is one line, and
            // two aliases pointing at the same box are otherwise indistinguishable here.
            item.toolTip = host.summary
            item.representedObject = host.alias
            menu.addItem(item)
        }
        if hosts.count > HostAppDelegate.sshMenuLimit {
            // Stated, not silently truncated. A menu that quietly drops hosts reads as a
            // config that lost them.
            let more = NSMenuItem(
                title: "\(hosts.count - HostAppDelegate.sshMenuLimit) more in cmd+K",
                action: nil, keyEquivalent: "")
            more.isEnabled = false
            menu.addItem(more)
        }

        menu.addItem(.separator())
        menu.addItem(
            withTitle: "SSH Connection...", action: #selector(menuSSHConnect), keyEquivalent: "")
        for item in menu.items where item.action != nil && item.target == nil {
            item.target = self
        }
        menu.popUp(positioning: nil, at: NSPoint(x: 0, y: view.bounds.minY - 4), in: view)
    }

    /// How many saved hosts the menu shows before deferring to the palette. A menu longer
    /// than the screen scrolls, and a scrolling menu is worse than a search box.
    private static let sshMenuLimit = 12

    @objc private func menuNewSession() { newSession() }

    @objc private func menuNewWorkspace() {
        // After the menu has torn itself down. A modal run from inside a dismissing menu
        // swallows its own keys, which is the same trap the palette hit.
        DispatchQueue.main.async { [weak self] in self?.newWorkspace() }
    }

    @objc private func menuSSHHost(_ item: NSMenuItem) {
        guard let alias = item.representedObject as? String else { return }
        openSSH(argv: ["ssh", alias], title: alias)
    }

    @objc private func menuSSHConnect() {
        DispatchQueue.main.async { [weak self] in self?.promptSSHConnection() }
    }

    /// The connection form, and what happens to what it collects.
    ///
    /// On save the ALIAS is dialled, not the assembled argv, so the pane runs the same
    /// thing a later `ssh <alias>` from any shell would run. If the two ever disagree, the
    /// config is wrong and the operator finds out immediately rather than the next time
    /// they use scp.
    private func promptSSHConnection() {
        switch SSHConnect.prompt(over: window) {
        case .cancelled:
            return
        case .connect(let connection):
            openSSH(
                argv: connection.arguments,
                title: connection.alias.isEmpty ? connection.hostName : connection.alias)
        case .saveAndConnect(let connection):
            switch SSHConfig.append(connection) {
            case .failure(let why):
                warn("Nothing was saved", why.summary)
            case .success(let alias):
                report("ssh: appended Host \(alias) to \(SSHConfig.defaultPath)")
                openSSH(argv: ["ssh", alias], title: alias)
            }
        }
    }

    // MARK: ssh hosts

    /// One palette row per concrete host in `~/.ssh/config`.
    ///
    /// Read at every palette open rather than cached: a host added to the config while the
    /// app is running should appear on the next cmd+K, and a cache here would be the copy
    /// that goes stale. The file is small and the palette is not a hot path.
    ///
    /// Nothing is offered when the file is absent, which is the normal state of a machine
    /// that has never used ssh. An empty section is better than a section of one dead row
    /// explaining what an ssh config is.
    private func sshPaletteItems() -> [PaletteItem] {
        SSHConfig.hosts().map { host in
            PaletteItem(
                title: "SSH: \(host.alias)", subtitle: host.summary, workflowIndex: nil,
                action: { [weak self] in self?.newSSHSession(host) })
        }
    }

    /// Opens a pane running `ssh <alias>`.
    ///
    /// The ALIAS is spawned, never the resolved hostname, user or port. Rebuilding an
    /// `ssh -p 2222 orel@host` line out of parsed fields would quietly drop every other
    /// directive the operator wrote - ProxyJump, IdentityFile, ForwardAgent - and it would
    /// drop them silently, so the connection either fails for no visible reason or, worse,
    /// succeeds over a path they did not intend.
    ///
    /// The alias is also not shell-quoted here because it is not going through a shell:
    /// `command` is the child's own line and the concrete-pattern filter has already
    /// refused anything with a glob character in it.
    private func newSSHSession(_ host: SSHHost) {
        openSSH(argv: ["ssh", host.alias], title: host.alias)
    }

    /// The ONE place an ssh child is spawned, and the one place its words are quoted.
    ///
    /// It takes argv, never a command line, because the host hands the string to
    /// `/bin/sh -c` and a caller that assembles its own line is one interpolation away from
    /// running an alias's semicolon. Every path in - a saved host, the form's Connect, the
    /// form's Save & Connect - comes through here so there is exactly one place that can
    /// be wrong about it.
    private func openSSH(argv: [String], title: String) {
        spawnCount += 1
        let (cols, rows) = gridForPane()
        let scale = Float(window.backingScaleFactor)
        guard
            let session = Session(
                command: SSHConnection.shellQuoted(argv), cols: cols, rows: rows,
                fontSize: baseFontSize * scale * fontScale,
                autoDirection: autoDirection, config: config, title: title)
        else {
            warn("Could not open a pane for \(title)", "The pty or the renderer refused.")
            return
        }
        sessions.append(session)
        activate(index: sessions.count - 1)
    }

    // MARK: workspaces (S5)

    /// Spawns a session placed in `directory`, labelled with its workspace.
    private func newSession(in directory: String, workspace: String) {
        spawnCount += 1
        let (cols, rows) = gridForPane()
        let scale = Float(window.backingScaleFactor)
        guard
            let session = Session(
                command: command, cols: cols, rows: rows,
                fontSize: baseFontSize * scale * fontScale,
                autoDirection: autoDirection, config: config, title: workspace,
                cwd: directory, workspace: workspace)
        else { return }
        sessions.append(session)
        activate(index: sessions.count - 1)
    }

    /// Asks for a name and creates a worktree plus a session in it.
    ///
    /// Modal on purpose. This is the app's only repository-mutating action, and a
    /// confirmation the operator cannot miss is the price of that (`Worktrees.swift`
    /// header). The prompt is also where the target path is shown, so nobody learns
    /// where their worktree went by finding it later.
    private func newWorkspace() {
        guard let directory = activeDirectory(),
            let root = Git.repositoryRoot(containing: directory)
        else {
            report("new workspace: the active session is not in a git repository")
            warn(
                "Not a git repository",
                "A workspace is a git worktree, so this session has to be inside a "
                    + "repository first.")
            return
        }

        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 320, height: 24))
        field.placeholderString = "branch name"
        let alert = NSAlert()
        alert.messageText = "New workspace"
        alert.informativeText =
            "Creates a git worktree beside \((root as NSString).lastPathComponent) and opens a "
            + "session in it. An existing branch is checked out; a new name creates the branch."
        alert.accessoryView = field
        alert.addButton(withTitle: "Create")
        alert.addButton(withTitle: "Cancel")
        alert.window.initialFirstResponder = field
        guard alert.runModal() == .alertFirstButtonReturn else { return }

        createWorkspace(root: root, branch: field.stringValue.trimmingCharacters(in: .whitespaces))
    }

    /// The half of `newWorkspace` past the prompt: validate, create, open a session.
    ///
    /// Split out so the live-tap hook drives the REAL path (worktree creation, the cwd
    /// spawn, the pill label) rather than a copy of it. The modal above is the only part
    /// the tap does not cover, and it is named as untested in the PR rather than implied.
    private func createWorkspace(root: String, branch: String) {
        if let invalid = Worktrees.validate(branch: branch) {
            warn("Cannot create that workspace", invalid.description)
            return
        }

        gitQueue.async { [weak self] in
            let outcome = Worktrees.add(root: root, branch: branch)
            DispatchQueue.main.async {
                guard let self else { return }
                switch outcome {
                case .success(let worktree):
                    self.report("workspace \(branch) at \(worktree.path)")
                    self.newSession(in: worktree.path, workspace: branch)
                case .failure(let error):
                    self.report("new workspace failed: \(error.description)")
                    self.warn("Could not create the workspace", error.description)
                }
            }
        }
    }

    /// Closes the active session and offers to remove its worktree.
    ///
    /// Two separate acts, and the session closes either way: a workspace with
    /// uncommitted work must still be closable without deleting it. git's refusal to
    /// remove a dirty tree is shown verbatim rather than forced past.
    private func closeWorkspace() {
        guard let session = activeSession, let workspace = session.workspace else {
            report("close workspace: the active session is not a workspace")
            return
        }
        guard let directory = activeDirectory(),
            let root = Git.repositoryRoot(containing: directory),
            case .success(let trees) = Worktrees.list(containing: directory),
            let worktree = trees.first(where: { $0.branch == workspace })
        else {
            report("close workspace: could not resolve the worktree for \(workspace)")
            closeSession(index: activeIndex)
            return
        }

        let alert = NSAlert()
        alert.messageText = "Close workspace \"\(workspace)\"?"
        alert.informativeText =
            "The session closes either way. Removing the worktree deletes \(worktree.path); "
            + "git refuses if it has uncommitted changes, and nothing here overrides that."
        alert.addButton(withTitle: "Close session only")
        alert.addButton(withTitle: "Close and remove worktree")
        alert.addButton(withTitle: "Cancel")
        let choice = alert.runModal()
        guard choice != .alertThirdButtonReturn else { return }

        closeSession(index: activeIndex)
        guard choice == .alertSecondButtonReturn else { return }

        gitQueue.async { [weak self] in
            let outcome = Worktrees.remove(root: root, worktree: worktree)
            DispatchQueue.main.async {
                guard let self else { return }
                if case .failure(let error) = outcome {
                    self.report("worktree remove refused: \(error.description)")
                    self.warn("The worktree was kept", error.description)
                } else {
                    self.report("removed worktree \(worktree.path)")
                }
            }
        }
    }

    private func warn(_ message: String, _ detail: String) {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = message
        alert.informativeText = detail
        alert.runModal()
    }

    private func closeSession(index: Int) {
        guard index >= 0 && index < sessions.count else { return }
        sessions[index].close()
        sessions.remove(at: index)
        if sessions.isEmpty {
            NSApp.terminate(nil)
            return
        }
        if activeIndex >= index {
            activate(index: max(0, activeIndex - 1))
        } else {
            refreshChrome()
        }
    }

    private func activate(index: Int) {
        guard index >= 0 && index < sessions.count else { return }
        activeIndex = index
        let session = sessions[index]
        session.markSeen()
        window.title = session.title
        refreshChrome()
        fitToPane(session)
        syncPresentTarget(to: session)
        showFrame(session.poll() ?? session.lastImage)
        applyBackground(of: session)
        // Mouse geometry is per-host state; a session activated (or just spawned)
        // after the last layout pass has never seen the view's size.
        view.pushMouseGeometry()
        // The ghost and its input tracking belong to the outgoing session's grid.
        hideGhost()
        lastInputLine = ""
        window.makeFirstResponder(view)
    }

    /// The margin around the grid wears the terminal's own background, so a future
    /// theme colors it without any app-side change.
    private func applyBackground(of session: Session) {
        guard let background = session.background else { return }
        view.layer?.backgroundColor = background
    }

    private func refreshChrome() {
        // A workspace session is labelled by its workspace, prefixed so the strip groups
        // by eye without a second row. The strip carried this alone between 2026-07-30 and
        // 2026-08-14, while there was no sidebar to group in; it still does, because a
        // sidebar row and a tab pill are read at different moments and the pill is the one
        // visible when the sidebar is undocked.
        let titles = sessions.map { session -> String in
            guard let workspace = session.workspace else { return session.title }
            return "\u{2387} \(workspace)"
        }
        tabBar.update(
            titles: titles,
            states: sessions.map(\.workState),
            activeIndex: activeIndex)
        guard sidebarDocked else { return }
        sidebar.update(rows: sessions.map(sidebarRow(for:)), activeIndex: activeIndex)
    }

    /// One session's row. The title half falls back to the session title so an ordinary
    /// shell is not a blank row; the subtitle half is allowed to be empty, because a
    /// session with no shell integration reports no directory and inventing one would be
    /// worse than showing none.
    private func sidebarRow(for session: Session) -> SidebarRow {
        let directory = directory(of: session)
        var parts: [String] = []
        if let directory, let branch = branch(in: directory) { parts.append(branch) }
        if let directory { parts.append(abbreviate(directory)) }
        return SidebarRow(
            title: session.workspace ?? session.title,
            subtitle: parts.joined(separator: " · "),
            state: session.workState)
    }

    /// `$HOME/x` renders as `~/x`. Purely cosmetic, and it is here rather than in the view
    /// because the view must not know what a home directory is.
    private func abbreviate(_ path: String) -> String {
        let home = NSHomeDirectory()
        guard path == home || path.hasPrefix(home + "/") else { return path }
        return "~" + path.dropFirst(home.count)
    }

    /// The branch checked out in `directory`, or nil when it is not a work tree.
    ///
    /// Cached forever within a run; see `branchCache`. A detached HEAD answers `HEAD`,
    /// which is git's own word for it and is left as-is rather than translated.
    private func branch(in directory: String) -> String? {
        if let cached = branchCache[directory] { return cached.isEmpty ? nil : cached }
        let result = Git.run(["rev-parse", "--abbrev-ref", "HEAD"], in: directory)
        let name =
            result.status == 0
            ? result.out.trimmingCharacters(in: .whitespacesAndNewlines) : ""
        // The empty string is cached too, so a non-repository directory costs one process
        // rather than one per refresh. This is the whole point of the cache.
        branchCache[directory] = name
        return name.isEmpty ? nil : name
    }

    // MARK: polling

    private func pollTick() {
        tick += 1
        guard let session = activeSession else { return }
        if session.presenting {
            session.present()
            applyBackground(of: session)
        }
        if let image = session.poll() {
            view.contentLayer.contents = image
            applyBackground(of: session)
        }
        // Outside the new-frame branch deliberately: a child that prints once and goes
        // quiet never yields another image, and a view left at cellHeight 0 has no
        // wheel handling at all (found by the SGR-mouse live tap). updateGutter's own
        // change guard makes the every-tick call free.
        view.updateGutter(
            blocks: computeBlocks(session.rowClasses),
            cellHeightDevice: session.cellHeight)
        // Events BEFORE the suggestion pass: the 133;C event and the cursor's move to
        // the fresh prompt land in the SAME polled frame, so recording must read the
        // previous tick's input line before captureAndSuggest overwrites it (the tap
        // caught exactly this race as an empty history file).
        apply(events: session.drainEvents(), to: session)
        captureAndSuggest(session)
        if !windowSized && session.cellWidth != 0 {
            windowSized = true
            sizeWindowToGrid(session)
        }
        if session.exited {
            closeSession(index: activeIndex)
            return
        }
        if tick % HostAppDelegate.backgroundEvery == 0 {
            reapBackgroundSessions()
        }
    }

    /// What the child asked its terminal to do. Policy lives here, not in the core:
    /// OSC 52 writes the system clipboard (write-only -- reads are refused core-side),
    /// notifications post through the user-notification center when we are a real
    /// bundle (the bare CLI has no identity to post as), BEL beeps.
    private func apply(events: [Session.HostEvent], to session: Session) {
        var chromeChanged = false
        for event in events {
            switch event {
            case .title(let text):
                session.applyTitle(text)
                chromeChanged = true
            case .progress(let state):
                session.apply(progress: state)
                chromeChanged = true
            case .clipboard(let text):
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(text, forType: .string)
            case .notify(let title, let body):
                postNotification(title: title.isEmpty ? "Mind2t" : title, body: body)
            case .bell:
                NSSound.beep()
            case .commandStart:
                // Execution began: the input line seen on the LAST tick is the final
                // typed command. Reading the row now would race the shell's redraw.
                recordExecutedCommand(of: session)
            case .pwd:
                // The session already stored it (`pwdRaw`). The consumer the old comment
                // here anticipated now exists: the sidebar's second line is the directory,
                // so a `cd` must repaint the chrome. Without this, a row keeps showing the
                // directory the session STARTED in, which is wrong in the one way that is
                // hardest to notice -- it is a real path, just not the current one.
                chromeChanged = true
            }
        }
        if chromeChanged {
            refreshChrome()
            if session === activeSession {
                window.title = session.title
            }
        }
    }

    private func postNotification(title: String, body: String) {
        guard Bundle.main.bundleIdentifier != nil else {
            FileHandle.standardError.write(Data("notify: \(title): \(body)\n".utf8))
            return
        }
        let center = UNUserNotificationCenter.current()
        center.requestAuthorization(options: [.alert, .sound]) { granted, _ in
            guard granted else { return }
            let content = UNMutableNotificationContent()
            content.title = title
            content.body = body
            center.add(
                UNNotificationRequest(
                    identifier: UUID().uuidString, content: content, trigger: nil))
        }
    }

    /// Low-rate sweep over the sessions not on screen: keep their images warm and drop
    /// the ones whose child exited. Walked backwards so removal keeps indices honest.
    private func reapBackgroundSessions() {
        for index in stride(from: sessions.count - 1, through: 0, by: -1)
        where index != activeIndex {
            let session = sessions[index]
            session.poll()
            // Background sessions still ring, notify and set the clipboard -- the
            // program asked; being off-screen is not a veto.
            apply(events: session.drainEvents(), to: session)
            if session.exited {
                sessions[index].close()
                sessions.remove(at: index)
                if activeIndex > index { activeIndex -= 1 }
            }
        }
        refreshChrome()
    }

    // MARK: geometry

    /// The grid that fits the terminal pane right now, or the spawn default before any
    /// cell metrics exist.
    private func gridForPane() -> (UInt16, UInt16) {
        guard let metrics = sessions.first(where: { $0.cellWidth != 0 }) else {
            return (80, 24)
        }
        let scale = window.backingScaleFactor
        let inner = view.bounds.insetBy(dx: TerminalView.padding, dy: TerminalView.padding)
        let cols = UInt16(max(2, Int(inner.width * scale) / metrics.cellWidth))
        let rows = UInt16(max(2, Int(inner.height * scale) / metrics.cellHeight))
        return (cols, rows)
    }

    private func fitToPane(_ session: Session) {
        guard session.cellWidth != 0, windowSized else { return }
        let (cols, rows) = gridForPane()
        session.resize(cols: cols, rows: rows)
    }

    /// Sizes the window once so the terminal pane is exactly the spawn grid, in points.
    private func sizeWindowToGrid(_ session: Session) {
        let scale = window.backingScaleFactor
        let size = NSSize(
            width: CGFloat(session.cellWidth * Int(session.cols)) / scale
                + TerminalView.padding * 2,
            height: CGFloat(session.cellHeight * Int(session.rows)) / scale
                + TerminalView.padding * 2 + TabBarView.height
        )
        window.setContentSize(size)
        window.contentResizeIncrements = NSSize(
            width: CGFloat(session.cellWidth) / scale,
            height: CGFloat(session.cellHeight) / scale)
        layoutChrome()
    }

    /// cmd+click: resolve the cell under the point and open its OSC 8 link, if any.
    private func openLink(at point: NSPoint) {
        guard let session = activeSession, session.cellWidth > 0 else { return }
        let scale = window.backingScaleFactor
        let x = (point.x - TerminalView.padding) * scale
        // AppKit grows upward; grid rows grow downward from the top padding edge.
        let yTop = (view.bounds.height - point.y - TerminalView.padding) * scale
        guard x >= 0, yTop >= 0 else { return }
        let col = UInt16(clamping: Int(x) / session.cellWidth)
        let row = UInt16(clamping: Int(yTop) / session.cellHeight)
        let uri = session.linkAt(col: col, row: row)
        // Permanent seam trace (SCAR-014): one line per cmd+click, so "nothing opened"
        // is always attributable to the event path, the cell math, or the lookup --
        // never a guess. Terminal apps live and die on their stderr being honest.
        FileHandle.standardError.write(
            Data("link: cell (\(col),\(row)) -> \(uri ?? "none")\n".utf8))
        guard let uri, let url = URL(string: uri) else { return }
        NSWorkspace.shared.open(url)
    }

    // MARK: zoom

    private func zoom(by factor: Float) { zoom(to: fontScale * factor) }

    /// The window keeps its pixel size; the grid that fits it moves with the metrics.
    /// Every session zooms together, so switching never changes the glyph size. Metrics
    /// for the NEW size come from the C query -- no session knows them until its next
    /// frame, which is too late to compute the grid.
    private func zoom(to scale: Float) {
        let clamped = min(max(scale, 0.5), 4.0)
        let device = window.backingScaleFactor
        let size = baseFontSize * Float(device) * clamped
        var cellW: UInt32 = 0
        var cellH: UInt32 = 0
        let metricsResult = fontFamily.withCStringOrNil { familyPointer in
            mind2t_host_cell_metrics(size, familyPointer, &cellW, &cellH)
        }
        guard metricsResult == MIND2T_HOST_SUCCESS,
            cellW > 0, cellH > 0
        else { return }
        fontScale = clamped
        let inner = view.bounds.insetBy(dx: TerminalView.padding, dy: TerminalView.padding)
        let cols = UInt16(max(2, Int(inner.width * device) / Int(cellW)))
        let rows = UInt16(max(2, Int(inner.height * device) / Int(cellH)))
        for session in sessions {
            session.setFontSize(size, cols: cols, rows: rows)
        }
        window.contentResizeIncrements = NSSize(
            width: CGFloat(cellW) / device, height: CGFloat(cellH) / device)
    }

    /// Re-lays the chrome on every resize step.
    ///
    /// REGRESSION FIXED HERE (found by the operator, 2026-08-02, the same day S5.5
    /// shipped): the chrome used to be laid out ONCE and kept correct during a resize by
    /// autoresizing masks. S5.5 removed those masks -- correctly, because `.width` on the
    /// pane stretches it past its computed frame -- and did not put anything back
    /// in their place, so the views simply kept their old frames while the
    /// window moved around them. The masks were half of a mechanism and only half was
    /// replaced.
    ///
    /// Fires continuously through a live resize, which is what the frames need. The pty
    /// deliberately does NOT follow at this rate; that stays at end-of-resize below.
    func windowDidResize(_ notification: Notification) {
        layoutChrome()
        // A drag is throttled to its end (below) because resizing the pty per step spams
        // SIGWINCH at the child. Everything else -- zoom, full screen, tiling, an
        // accessibility client setting the size -- produces no live-resize session at
        // all, so `windowDidEndLiveResize` never fires and the pty would keep the old
        // grid indefinitely. Found by resizing through the accessibility API while
        // verifying the fix above, which is exactly the case that does not fire.
        if !view.inLiveResize {
            refitAll()
        }
    }

    func windowDidEndLiveResize(_ notification: Notification) {
        // Every session, not just the active one: a background session that missed the
        // resize repaints at the old geometry the moment it is activated.
        refitAll()
    }

    func windowWillClose(_ notification: Notification) {
        NSApp.terminate(nil)
    }

    func applicationWillTerminate(_ notification: Notification) {
        timer?.invalidate()
        for session in sessions {
            session.close()
        }
        sessions.removeAll()
    }

    // MARK: blocks (S2)

    /// The clicked block's actions. The menu is built per click -- blocks move as the
    /// grid scrolls, so caching one would act on stale rows.
    // MARK: autosuggestions (S4)

    /// The command history handle, app-global. Loaded once; appends persist as they
    /// happen, so a crash costs at most the last command.
    private var historyStore: OpaquePointer?
    private let ghost = CATextLayer()
    /// The un-typed remainder of the current suggestion, ready for the paste path.
    private var ghostRemainder: [UInt8] = []
    /// The cursor row's input text as of the last tick, per session identity: the
    /// OSC 133;C event says WHEN a command executed, and this is WHAT it was -- read
    /// a tick earlier, because by event time the shell may already be redrawing.
    private var lastInputLine = ""

    private func setupSuggestions() {
        if let configDir {
            let path = (configDir as NSString).appendingPathComponent("history")
            _ = path.withCString { pointer in mind2t_history_load(pointer, &historyStore) }
        } else {
            _ = mind2t_history_load(nil, &historyStore)
        }
        ghost.actions = [
            "contents": NSNull(), "position": NSNull(), "bounds": NSNull(),
            "hidden": NSNull(), "string": NSNull(),
        ]
        ghost.foregroundColor = NSColor(white: 1, alpha: 0.35).cgColor
        ghost.isHidden = true
        view.contentLayer.addSublayer(ghost)
    }

    /// The typed text on the caret's row -- input-marked cells only, so the prompt
    /// itself never leaks in. Single-row deliberately: a wrapped command is a named
    /// v1 boundary, and joining a whole prompt RUN would merge adjacent commands
    /// (the cd-then-cd case measured in the tap).
    private func currentInputLine(_ session: Session) -> String {
        session.rowText(UInt16(session.cursorRow), semantic: UInt8(MIND2T_ROW_INPUT))
            .trimmingCharacters(in: .whitespaces)
    }

    /// The event half: OSC 133;C fired, so the last tick's input line IS the command.
    private func recordExecutedCommand(of session: Session) {
        guard session === activeSession, !lastInputLine.isEmpty else { return }
        session.recordCommand(historyStore, command: lastInputLine)
        lastInputLine = ""
    }

    /// Per tick: remember what is typed (for the C event) and refresh the ghost.
    /// Everything rides the OSC 133 rails -- no shell integration means no input
    /// marks, no history, no suggestions: the S2 dependency the backlog names.
    private func captureAndSuggest(_ session: Session) {
        let input = currentInputLine(session)
        lastInputLine = input
        updateGhost(session, input: input)
    }

    private func updateGhost(_ session: Session, input: String) {
        // Only at the live bottom, with a visible caret parked at the END of the
        // typed line -- a ghost mid-line would suggest an edit we cannot make.
        //
        // AT OR PAST the end, not exactly at it. `mind2t_host_row_text` ends with
        // `trim_end_matches(' ')`, so spaces the operator actually typed are
        // invisible here: typing `echo ` puts the caret at column 7 against a
        // reported length of 6 and the old equality hid the ghost. Measured by
        // live tap 2026-07-31 -- `echo a` suggested, `echo ` did not, which made
        // the trailing space the single most common way to see nothing, since a
        // space is what you type before an argument.
        let rowText = session.rowText(
            UInt16(session.cursorRow), semantic: UInt8(MIND2T_TEXT_ALL))
        guard session.viewportOffset == 0, session.cursorVisible,
            session.cellWidth > 0, !input.isEmpty,
            session.cursorCol >= rowText.count
        else { return hideGhost() }

        // Keyed by the directory the session last reported (OSC 7): a command run HERE
        // outranks a newer one run elsewhere, and the host does the decoding.
        guard let suggestion = session.suggestion(historyStore, for: input) else {
            return hideGhost()
        }
        var remainder = String(suggestion.dropFirst(input.count))
        // Those same invisible spaces have to come off the FRONT of the remainder,
        // or the ghost redraws a space the caret is already sitting past and the
        // line reads `echo  alpha-here`. Bounded by how many the caret implies, so
        // a suggestion whose own next character is a space is not eaten.
        var typedSpaces = session.cursorCol - rowText.count
        while typedSpaces > 0, remainder.first == " " {
            remainder.removeFirst()
            typedSpaces -= 1
        }
        guard !remainder.isEmpty else { return hideGhost() }

        ghostRemainder = Array(remainder.utf8)
        let scale = window.backingScaleFactor
        let cellWidth = CGFloat(session.cellWidth) / scale
        let cellHeight = CGFloat(session.cellHeight) / scale
        let fontSize = CGFloat(baseFontSize * fontScale)
        ghost.font = NSFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
        ghost.fontSize = fontSize
        ghost.contentsScale = scale
        ghost.string = remainder
        ghost.frame = CGRect(
            x: CGFloat(session.cursorCol) * cellWidth,
            y: view.contentLayer.bounds.height - CGFloat(session.cursorRow + 1) * cellHeight,
            width: CGFloat(remainder.count) * cellWidth + cellWidth,
            height: cellHeight)
        ghost.isHidden = false
    }

    private func hideGhost() {
        ghost.isHidden = true
        ghostRemainder = []
    }

    // MARK: command palette (S3)

    private var palette: PaletteView?

    /// cmd+K toggles. Items are rebuilt and workflows reloaded at every open, so
    /// session changes and template file edits show up without a restart.
    private func togglePalette() {
        if let palette {
            palette.removeFromSuperview()
            self.palette = nil
            window.makeFirstResponder(view)
            return
        }
        let workflows = Workflows(
            dir: configDir.map { ($0 as NSString).appendingPathComponent("workflows") })
        var items: [PaletteItem] = [
            PaletteItem(
                title: "New Session", subtitle: "cmd+T", workflowIndex: nil,
                action: { [weak self] in self?.newSession() }),
            PaletteItem(
                title: "Close Session", subtitle: "cmd+W", workflowIndex: nil,
                action: { [weak self] in
                    guard let self, self.activeIndex >= 0 else { return }
                    self.closeSession(index: self.activeIndex)
                }),
        ]
        // Above workspaces: launching an agent is the thing this terminal exists for, and a
        // palette is read from the top.
        items.append(contentsOf: agentPaletteItems())
        items.append(contentsOf: sshPaletteItems())
        items.append(
            PaletteItem(
                title: "New Workspace", subtitle: "a git worktree and a session in it",
                workflowIndex: nil,
                // After the palette has torn itself down: both of these open a modal,
                // and a modal run from inside the dismissing view swallows its own keys.
                action: { [weak self] in
                    DispatchQueue.main.async { self?.newWorkspace() }
                }))
        if activeSession?.workspace != nil {
            items.append(
                PaletteItem(
                    title: "Close Workspace", subtitle: "session, and optionally the worktree",
                    workflowIndex: nil,
                    action: { [weak self] in
                        DispatchQueue.main.async { self?.closeWorkspace() }
                    }))
        }
        if view.onDiffPanel != nil {
            items.append(
                PaletteItem(
                    title: "Review Changes", subtitle: "cmd+shift+D", workflowIndex: nil,
                    action: { [weak self] in
                        // After the palette has dismissed itself, or the panel would be
                        // added under a view that is about to be torn down.
                        DispatchQueue.main.async { self?.toggleDiffPanel() }
                    }))
        }
        for (index, session) in sessions.enumerated() where index != activeIndex {
            items.append(
                PaletteItem(
                    title: "Switch: \(session.title)", subtitle: "session \(index + 1)",
                    workflowIndex: nil,
                    action: { [weak self] in self?.activate(index: index) }))
        }
        if let session = activeSession,
            let block = computeBlocks(session.rowClasses).last
        {
            let command = session.command(of: block)
            if !command.isEmpty {
                items.append(
                    PaletteItem(
                        title: "Copy Last Command", subtitle: command, workflowIndex: nil,
                        action: { [weak self] in
                            guard let self, let session = self.activeSession else { return }
                            self.clip(session.command(of: block))
                        }))
                items.append(
                    PaletteItem(
                        title: "Copy Last Output", subtitle: "the block's output text",
                        workflowIndex: nil,
                        action: { [weak self] in
                            guard let self, let session = self.activeSession else { return }
                            self.clip(session.output(of: block))
                        }))
            }
        }
        items.append(
            PaletteItem(
                title: "Scroll to Top", subtitle: "cmd+Home", workflowIndex: nil,
                action: { [weak self] in self?.activeSession?.scroll(Int32.max / 2) }))
        items.append(
            PaletteItem(
                title: "Scroll to Bottom", subtitle: "cmd+End", workflowIndex: nil,
                action: { [weak self] in self?.activeSession?.scroll(Int32.min) }))
        for index in 0..<workflows.count {
            let description = workflows.field(index, MIND2T_WORKFLOW_DESCRIPTION)
            let command = workflows.field(index, MIND2T_WORKFLOW_COMMAND)
            items.append(
                PaletteItem(
                    title: workflows.field(index, MIND2T_WORKFLOW_NAME),
                    subtitle: description.isEmpty ? command : description,
                    workflowIndex: index, action: nil))
        }
        let errors = workflows.errors
        if !errors.isEmpty {
            items.append(
                PaletteItem(
                    title: "\u{26A0} Workflow files with errors",
                    subtitle: errors.replacingOccurrences(of: "\n", with: " \u{2022} "),
                    workflowIndex: nil,
                    action: {
                        let alert = NSAlert()
                        alert.alertStyle = .warning
                        alert.messageText = "Some workflow files were skipped"
                        alert.informativeText = errors
                        alert.runModal()
                    }))
        }

        let paletteView = PaletteView(items: items, workflows: workflows)
        paletteView.onDismiss = { [weak self] in
            guard let self else { return }
            self.palette?.removeFromSuperview()
            self.palette = nil
            self.window.makeFirstResponder(self.view)
        }
        paletteView.onCommand = { [weak self] bytes in
            guard let session = self?.activeSession else { return }
            session.scroll(Int32.min)
            session.paste(bytes)
        }
        paletteView.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(paletteView)
        NSLayoutConstraint.activate([
            paletteView.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            paletteView.topAnchor.constraint(equalTo: view.topAnchor, constant: 64),
            paletteView.widthAnchor.constraint(equalToConstant: 560),
        ])
        palette = paletteView
        paletteView.focus()
    }

    // MARK: diff review panel (S6)

    /// The active session's working directory, decoded.
    private func activeDirectory() -> String? {
        guard let session = activeSession else { return nil }
        return directory(of: session)
    }

    /// Any session's working directory, decoded.
    ///
    /// Decoded by the RUST side (`mind2t_cwd_path`), never here: the percent-decode and
    /// the `file://host` strip already exist in one place, and a second copy in Swift
    /// is exactly the drift the OSC 7 slice was written to avoid.
    ///
    /// Generalised from `activeDirectory` for the sidebar, which needs the directory of
    /// every session rather than one. The decoding is untouched; only the session it
    /// reads from became a parameter.
    private func directory(of session: Session) -> String? {
        guard !session.pwdRaw.isEmpty else { return nil }
        var length = 0
        let raw = session.pwdRaw
        let sized = raw.withUnsafeBufferPointer { pointer in
            mind2t_cwd_path(pointer.baseAddress, raw.count, nil, 0, &length)
        }
        guard sized == MIND2T_HOST_SUCCESS, length > 0 else { return nil }
        var buffer = [UInt8](repeating: 0, count: length)
        let copied = raw.withUnsafeBufferPointer { pointer in
            buffer.withUnsafeMutableBufferPointer { out in
                mind2t_cwd_path(pointer.baseAddress, raw.count, out.baseAddress, length, &length)
            }
        }
        guard copied == MIND2T_HOST_SUCCESS else { return nil }
        return String(decoding: buffer, as: UTF8.self)
    }

    /// The panel wears the terminal's own background; the rest is the app's accent, the
    /// same indigo the gutter's hot bar uses. Only the background is live today -- the
    /// frame reports it, and nothing reports a foreground yet.
    private func panelTheme() -> PanelTheme {
        var background = "#16151b"
        if let color = activeSession?.background,
            let srgb = NSColor(cgColor: color)?.usingColorSpace(.sRGB)
        {
            background = String(
                format: "#%02x%02x%02x", Int((srgb.redComponent * 255).rounded()),
                Int((srgb.greenComponent * 255).rounded()),
                Int((srgb.blueComponent * 255).rounded()))
        }
        return PanelTheme(
            background: background, foreground: "#e6e6e6", accent: "#5865f2", dim: "#8b8b96")
    }

    /// What a panel paints before its document has, matching the CSS `--panel`
    /// (`color-mix(in srgb, var(--bg) 88%, white 12%)`).
    ///
    /// Duplicated arithmetic, deliberately and narrowly: the container has to have a
    /// colour BEFORE any message reaches the document, so it cannot come from the theme
    /// the document is sent. Keeping the two in step is the reason the mix is written
    /// out here rather than approximated.
    private func panelBackground() -> NSColor {
        let base =
            activeSession?.background.flatMap { NSColor(cgColor: $0) }?
            .usingColorSpace(.sRGB) ?? NSColor(srgbRed: 0.086, green: 0.082, blue: 0.106, alpha: 1)
        return NSColor(
            srgbRed: base.redComponent * 0.88 + 0.12,
            green: base.greenComponent * 0.88 + 0.12,
            blue: base.blueComponent * 0.88 + 0.12, alpha: 1)
    }

    /// cmd+shift+D toggles. Only ever reachable when `panels = true`.
    private func toggleDiffPanel() {
        if let webPanel {
            webPanel.removeFromSuperview()
            self.webPanel = nil
            panelRoot = nil
            panelFiles = []
            window.makeFirstResponder(view)
            return
        }
        guard let url = WebPanel.documentURL(override: webDir) else {
            report("panels are enabled but the panel document was not found (build web/)")
            return
        }
        guard let panel = WebPanel(documentURL: url, background: panelBackground()) else { return }
        panel.onProtocolError = { [weak self] detail in self?.report("panel bridge: \(detail)") }
        panel.onMessage = { [weak self] message in self?.handle(message) }
        panel.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(panel)
        // 1080pt wide where there is room, otherwise the window minus a margin. The
        // preferred width is breakable so the required inequality wins on a narrow
        // window instead of the layout engine reporting a conflict.
        let preferredWidth = panel.widthAnchor.constraint(equalToConstant: 1080)
        preferredWidth.priority = .defaultHigh
        NSLayoutConstraint.activate([
            panel.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            panel.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            panel.widthAnchor.constraint(
                lessThanOrEqualTo: view.widthAnchor, constant: -48),
            preferredWidth,
            panel.heightAnchor.constraint(equalTo: view.heightAnchor, multiplier: 0.82),
        ])
        webPanel = panel
        panel.focus()
    }





    /// Every session is refitted, not just the active one: a background session keeps
    /// running with its own geometry, and one that missed the dock would repaint at the
    /// old width the moment it is activated.
    private func refitAll() {
        for session in sessions {
            fitToPane(session)
        }
        showFrame(activeSession?.poll() ?? activeSession?.lastImage)
    }


    /// Focuses the session already living in `path`, or opens one there.
    ///
    /// The path is checked against the worktrees this host listed, for the same reason
    /// the diff panel checks its file paths: an argument that crosses the boundary is
    /// validated by the side that acts on it, and acting here means spawning a shell.
    private func openWorkspace(path: String) {
        guard let directory = activeDirectory(),
            case .success(let trees) = Worktrees.list(containing: directory),
            let worktree = trees.first(where: { $0.path == path })
        else {
            report("asked to open an unlisted worktree: \(path)")
            return
        }
        if let index = sessions.firstIndex(where: { $0.workspace == worktree.branch }) {
            activate(index: index)
            return
        }
        newSession(in: worktree.path, workspace: worktree.label)
    }


    private func handle(_ message: PanelMessage) {
        switch message {
        case .ready:
            webPanel?.post(.initialize(theme: panelTheme(), panel: .diff))
            refreshDiffPanel()
        case .refresh:
            refreshDiffPanel()
        case .dismiss:
            if webPanel != nil { toggleDiffPanel() }
        case .requestDiff(let path):
            sendDiff(path: path)
        case .openWorkspace, .createWorkspace:
            break
        case .pong:
            // Only the smoke asserts on this; the window has nothing to do with it.
            break
        case .decodeError(let detail):
            report("panel could not read a host message: \(detail)")
        }
    }

    /// Recomputes the changed-file list off the main thread and posts it.
    private func refreshDiffPanel() {
        guard let directory = activeDirectory() else {
            panelRoot = nil
            panelFiles = []
            webPanel?.post(.files(repo: nil, files: [], error: nil))
            return
        }
        gitQueue.async { [weak self] in
            let root = Git.repositoryRoot(containing: directory)
            let outcome = root.map { Git.changedFiles(root: $0) }
            DispatchQueue.main.async {
                guard let self, self.webPanel != nil else { return }
                self.panelRoot = root
                switch outcome {
                case .none:
                    self.panelFiles = []
                    self.webPanel?.post(.files(repo: nil, files: [], error: nil))
                case .some(.success(let files)):
                    self.panelFiles = files
                    self.webPanel?.post(.files(repo: root, files: files, error: nil))
                case .some(.failure(let error)):
                    self.panelFiles = []
                    self.webPanel?.post(.files(repo: root, files: [], error: error.description))
                }
            }
        }
    }

    /// Answers one `requestDiff`.
    ///
    /// The path is checked against the list this host advertised. That is not paranoia
    /// about our own React: it is the IPC rule -- an argument arriving from the other
    /// side of a boundary is validated by the side that acts on it, and here acting
    /// means reading a file off disk.
    private func sendDiff(path: String) {
        guard let root = panelRoot, let file = panelFiles.first(where: { $0.path == path }) else {
            report("panel asked for a diff of an unlisted path: \(path)")
            webPanel?.post(.fileDiff(path: path, patch: "", error: "not a listed change"))
            return
        }
        let untracked = file.status.hasPrefix("??")
        gitQueue.async { [weak self] in
            let outcome = Git.diff(root: root, path: path, untracked: untracked)
            DispatchQueue.main.async {
                guard let self, self.webPanel != nil else { return }
                switch outcome {
                case .success(let patch):
                    self.webPanel?.post(.fileDiff(path: path, patch: patch, error: nil))
                case .failure(let error):
                    self.webPanel?.post(
                        .fileDiff(path: path, patch: "", error: error.description))
                }
            }
        }
    }

    /// One line to stderr per seam event (SCAR-014): a panel that shows nothing must be
    /// distinguishable from a panel that was never asked anything.
    private func report(_ detail: String) {
        FileHandle.standardError.write(Data("[panel] \(detail)\n".utf8))
    }

    private func showBlockMenu(_ block: Block, with event: NSEvent) {
        guard let session = activeSession else { return }
        let command = session.command(of: block)

        let menu = NSMenu()
        // Explicit enablement: auto-enablement re-validates through the responder chain
        // and silently turns the flags below back on.
        menu.autoenablesItems = false
        // The command as a disabled header, so the menu names what it acts on.
        let header = NSMenuItem(
            title: command.isEmpty ? "Block" : "$ \(command)", action: nil, keyEquivalent: "")
        header.isEnabled = false
        menu.addItem(header)
        menu.addItem(.separator())

        let copyCommand = NSMenuItem(
            title: "Copy Command", action: #selector(blockCopyCommand(_:)), keyEquivalent: "")
        let copyOutput = NSMenuItem(
            title: "Copy Output", action: #selector(blockCopyOutput(_:)), keyEquivalent: "")
        let rerun = NSMenuItem(
            title: "Run Again", action: #selector(blockRerun(_:)), keyEquivalent: "")
        for item in [copyCommand, copyOutput, rerun] {
            item.target = self
            item.representedObject = BlockRef(block: block)
            menu.addItem(item)
        }
        copyCommand.isEnabled = !command.isEmpty
        rerun.isEnabled = !command.isEmpty
        NSMenu.popUpContextMenu(menu, with: event, for: view)
    }

    /// NSMenuItem.representedObject wants a class; Block is a value.
    private final class BlockRef: NSObject {
        let block: Block
        init(block: Block) { self.block = block }
    }

    private func clip(_ text: String) {
        guard !text.isEmpty else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    @objc private func blockCopyCommand(_ sender: NSMenuItem) {
        guard let reference = sender.representedObject as? BlockRef,
            let session = activeSession
        else { return }
        clip(session.command(of: reference.block))
    }

    @objc private func blockCopyOutput(_ sender: NSMenuItem) {
        guard let reference = sender.representedObject as? BlockRef,
            let session = activeSession
        else { return }
        clip(session.output(of: reference.block))
    }

    @objc private func blockRerun(_ sender: NSMenuItem) {
        guard let reference = sender.representedObject as? BlockRef,
            let session = activeSession
        else { return }
        let command = session.command(of: reference.block)
        guard !command.isEmpty else { return }
        session.send(Array(command.utf8) + [0x0d])
    }

    // MARK: SessionListDelegate

    func tabBarDidSelect(index: Int) {
        activate(index: index)
    }

    func tabBarDidRequestNew() {
        newSession()
    }

    func tabBarDidRequestClose(index: Int) {
        closeSession(index: index)
    }

    // MARK: SidebarDelegate

    /// Selection only. A sidebar row deliberately does NOT close or create: the strip
    /// already owns both, and two places to destroy a session is how one of them ends up
    /// with a stale index after the other has renumbered.
    func sidebarDidSelect(index: Int) {
        activate(index: index)
    }

}
