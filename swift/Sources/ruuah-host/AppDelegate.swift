// The app: a sidebar of sessions beside one terminal surface.
//
// Each session is its own RuuahHost -- pty, pump thread, renderer -- and the app blits
// whichever is active. Background sessions keep running and are polled at a low rate
// only to notice exits; their frames queue in the seqlock (the writer never blocks), so
// a switch shows the newest state immediately.

import AppKit
import CRuuahHost

final class HostAppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate,
    SessionListDelegate
{
    private let command: String?
    private let autoDirection: Bool
    /// The loaded settings handle; contributes the theme palette at every spawn. Owned
    /// for the app's lifetime (the borrowed strings read at launch depend on it).
    private let config: OpaquePointer?
    /// Logical font size from the config (or the default); scaled per spawn.
    private let baseFontSize: Float
    /// What could not be honoured in the config, shown once at launch -- a settings file
    /// that silently half-applies looks like a broken app, so the failure is loud.
    private let configError: String?
    private var window: NSWindow!
    private var view: TerminalView!
    private var sidebar: SessionListController!
    private var split: NSSplitView!
    private var timer: Timer?

    private var sessions: [Session] = []
    private var activeIndex = -1
    private var spawnCount = 0
    private var windowSized = false
    private var tick: UInt64 = 0
    /// Background sessions are polled once per this many active ticks (~2 Hz at 60).
    private static let backgroundEvery: UInt64 = 30

    init(
        command: String?, autoDirection: Bool, config: OpaquePointer?,
        baseFontSize: Float, configError: String?
    ) {
        self.command = command
        self.autoDirection = autoDirection
        self.config = config
        self.baseFontSize = baseFontSize
        self.configError = configError
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        view = TerminalView(frame: .zero)
        view.wantsLayer = true

        sidebar = SessionListController()
        sidebar.delegate = self

        split = NSSplitView()
        split.isVertical = true
        split.dividerStyle = .thin
        split.addArrangedSubview(sidebar.view)
        split.addArrangedSubview(view)
        // The sidebar keeps its width; the terminal pane absorbs every resize.
        sidebar.view.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        split.setHoldingPriority(.init(260), forSubviewAt: 0)
        split.setHoldingPriority(.init(250), forSubviewAt: 1)

        window = NSWindow(
            contentRect: NSRect(
                x: 0, y: 0, width: 800 + SessionListController.width, height: 480),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title =
            (Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String)
            ?? "ruuah-vt host"
        window.contentView = split
        window.delegate = self
        window.center()
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(view)
        NSApp.activate(ignoringOtherApps: true)
        split.setPosition(SessionListController.width, ofDividerAt: 0)

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

        view.onKeyBytes = { [weak self] bytes in self?.activeSession?.send(bytes) }
        view.onPaste = { [weak self] bytes in self?.activeSession?.paste(bytes) }
        view.onNewSession = { [weak self] in self?.newSession() }
        view.onCloseSession = { [weak self] in
            guard let self, self.activeIndex >= 0 else { return }
            self.closeSession(index: self.activeIndex)
        }
        view.onBlockClick = { [weak self] block, event in
            self?.showBlockMenu(block, with: event)
        }

        newSession()
        guard !sessions.isEmpty else {
            NSApp.terminate(nil)
            return
        }

        // For screenshots: `screencapture -l` takes this id. Written unbuffered, because a
        // scripted capture reads it while the app is still running.
        FileHandle.standardOutput.write(
            Data("RUUAH_HOST_WINDOW=\(window.windowNumber)\n".utf8))

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
                command: command, cols: cols, rows: rows, fontSize: baseFontSize * scale,
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
        window.title = session.title
        refreshSidebar()
        fitToPane(session)
        if let image = session.poll() ?? session.lastImage {
            view.contentLayer.contents = image
        }
        applyBackground(of: session)
        window.makeFirstResponder(view)
    }

    /// The margin around the grid wears the terminal's own background, so a future
    /// theme colors it without any app-side change.
    private func applyBackground(of session: Session) {
        guard let background = session.background else { return }
        view.layer?.backgroundColor = background
    }

    private func refreshSidebar() {
        sidebar.update(titles: sessions.map(\.title), activeIndex: activeIndex)
    }

    // MARK: polling

    private func pollTick() {
        tick += 1
        guard let session = activeSession else { return }
        if let image = session.poll() {
            view.contentLayer.contents = image
            applyBackground(of: session)
            view.updateGutter(
                blocks: computeBlocks(session.rowClasses),
                cellHeightDevice: session.cellHeight)
        }
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

    /// Low-rate sweep over the sessions not on screen: keep their images warm and drop
    /// the ones whose child exited. Walked backwards so removal keeps indices honest.
    private func reapBackgroundSessions() {
        for index in stride(from: sessions.count - 1, through: 0, by: -1)
        where index != activeIndex {
            let session = sessions[index]
            session.poll()
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
                + TerminalView.padding * 2 + SessionListController.width
                + split.dividerThickness,
            height: CGFloat(session.cellHeight * Int(session.rows)) / scale
                + TerminalView.padding * 2
        )
        window.setContentSize(size)
        window.contentResizeIncrements = NSSize(
            width: CGFloat(session.cellWidth) / scale,
            height: CGFloat(session.cellHeight) / scale)
        split.setPosition(SessionListController.width, ofDividerAt: 0)
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

    func sessionListDidSelect(index: Int) {
        activate(index: index)
    }

    func sessionListDidRequestNew() {
        newSession()
    }

    func sessionListDidRequestClose(index: Int) {
        closeSession(index: index)
    }
}
