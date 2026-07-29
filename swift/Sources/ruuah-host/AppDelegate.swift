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

    init(command: String?, autoDirection: Bool) {
        self.command = command
        self.autoDirection = autoDirection
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

        view.onKeyBytes = { [weak self] bytes in self?.activeSession?.send(bytes) }
        view.onPaste = { [weak self] bytes in self?.activeSession?.paste(bytes) }
        view.onNewSession = { [weak self] in self?.newSession() }
        view.onCloseSession = { [weak self] in
            guard let self, self.activeIndex >= 0 else { return }
            self.closeSession(index: self.activeIndex)
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
                command: command, cols: cols, rows: rows, fontSize: 16 * scale,
                autoDirection: autoDirection, title: title)
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
        window.makeFirstResponder(view)
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
