// The app: a top tab strip over one terminal surface.
//
// Each session is its own RuuahHost -- pty, pump thread, renderer -- and the app blits
// whichever is active. Background sessions keep running and are polled at a low rate
// only to notice exits; their frames queue in the seqlock (the writer never blocks), so
// a switch shows the newest state immediately.

import AppKit
import CRuuahHost
import UserNotifications

final class HostAppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate,
    TabBarDelegate
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
    private var window: NSWindow!
    private var view: TerminalView!
    private var tabBar: TabBarView!
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

    init(
        command: String?, autoDirection: Bool, config: OpaquePointer?,
        baseFontSize: Float, configError: String?, configDir: String? = nil
    ) {
        self.command = command
        self.autoDirection = autoDirection
        self.config = config
        self.baseFontSize = baseFontSize
        self.configError = configError
        self.configDir = configDir
        if let config, let family = ruuah_config_font_family(config) {
            self.fontFamily = String(cString: family)
        } else {
            self.fontFamily = nil
        }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        view = TerminalView(frame: .zero)
        view.wantsLayer = true

        tabBar = TabBarView(frame: .zero)
        tabBar.delegate = self

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
            ?? "ruuah-vt host"
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden

        let content = NSView(frame: window.contentLayoutRect)
        content.autoresizesSubviews = true
        content.addSubview(tabBar)
        content.addSubview(view)
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
        view.layer?.addSublayer(view.contentLayer)
        view.contentLayer.contentsScale = window.backingScaleFactor
        view.contentLayer.magnificationFilter = .nearest

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
            Data("RUUAH_HOST_WINDOW=\(window.windowNumber)\n".utf8))
        // The frame too, top-left origin, so scripted captures and synthetic-input
        // tests (SCAR-014 live taps) can aim without a window-server query.
        let screenHeight = window.screen?.frame.height ?? 0
        let frame = window.frame
        FileHandle.standardOutput.write(
            Data(
                "RUUAH_HOST_FRAME=\(Int(frame.origin.x)),\(Int(screenHeight - frame.origin.y - frame.height)),\(Int(frame.width)),\(Int(frame.height))\n"
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
        let bounds = content.bounds
        tabBar.frame = NSRect(
            x: 0, y: bounds.height - TabBarView.height,
            width: bounds.width, height: TabBarView.height)
        tabBar.autoresizingMask = [.width, .minYMargin]
        view.frame = NSRect(
            x: 0, y: 0, width: bounds.width, height: bounds.height - TabBarView.height)
        view.autoresizingMask = [.width, .height]
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
            refreshSidebar()
        }
    }

    private func activate(index: Int) {
        guard index >= 0 && index < sessions.count else { return }
        activeIndex = index
        let session = sessions[index]
        session.markSeen()
        window.title = session.title
        refreshSidebar()
        fitToPane(session)
        if let image = session.poll() ?? session.lastImage {
            view.contentLayer.contents = image
        }
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

    private func refreshSidebar() {
        tabBar.update(
            titles: sessions.map(\.title),
            states: sessions.map(\.workState),
            activeIndex: activeIndex)
    }

    // MARK: polling

    private func pollTick() {
        tick += 1
        guard let session = activeSession else { return }
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
                postNotification(title: title.isEmpty ? "RUUAH VT" : title, body: body)
            case .bell:
                NSSound.beep()
            case .commandStart:
                // Execution began: the input line seen on the LAST tick is the final
                // typed command. Reading the row now would race the shell's redraw.
                recordExecutedCommand(of: session)
            }
        }
        if chromeChanged {
            refreshSidebar()
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
        refreshSidebar()
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
            ruuah_host_cell_metrics(size, familyPointer, &cellW, &cellH)
        }
        guard metricsResult == RUUAH_HOST_SUCCESS,
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

    func windowDidEndLiveResize(_ notification: Notification) {
        guard let session = activeSession else { return }
        fitToPane(session)
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
            _ = path.withCString { pointer in ruuah_history_load(pointer, &historyStore) }
        } else {
            _ = ruuah_history_load(nil, &historyStore)
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
        session.rowText(UInt16(session.cursorRow), semantic: UInt8(RUUAH_ROW_INPUT))
            .trimmingCharacters(in: .whitespaces)
    }

    /// The event half: OSC 133;C fired, so the last tick's input line IS the command.
    private func recordExecutedCommand(of session: Session) {
        guard session === activeSession, !lastInputLine.isEmpty else { return }
        _ = Array(lastInputLine.utf8).withUnsafeBufferPointer { pointer in
            ruuah_history_append(historyStore, pointer.baseAddress, pointer.count)
        }
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
        guard session.viewportOffset == 0, session.cursorVisible,
            session.cellWidth > 0, !input.isEmpty,
            session.cursorCol
                == session.rowText(
                    UInt16(session.cursorRow), semantic: UInt8(RUUAH_TEXT_ALL)
                ).count
        else { return hideGhost() }

        var length = 0
        let inputBytes = Array(input.utf8)
        let sized = inputBytes.withUnsafeBufferPointer { pointer in
            ruuah_history_suggest(historyStore, pointer.baseAddress, pointer.count, nil, 0, &length)
        }
        guard sized == RUUAH_HOST_SUCCESS, length > 0 else { return hideGhost() }
        var buffer = [UInt8](repeating: 0, count: length)
        let copied = inputBytes.withUnsafeBufferPointer { pointer in
            buffer.withUnsafeMutableBufferPointer { outPointer in
                ruuah_history_suggest(
                    historyStore, pointer.baseAddress, pointer.count,
                    outPointer.baseAddress, length, &length)
            }
        }
        guard copied == RUUAH_HOST_SUCCESS else { return hideGhost() }
        let suggestion = String(decoding: buffer, as: UTF8.self)
        let remainder = String(suggestion.dropFirst(input.count))
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
            let description = workflows.field(index, RUUAH_WORKFLOW_DESCRIPTION)
            let command = workflows.field(index, RUUAH_WORKFLOW_COMMAND)
            items.append(
                PaletteItem(
                    title: workflows.field(index, RUUAH_WORKFLOW_NAME),
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
}
