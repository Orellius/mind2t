// One terminal session: a host handle, its geometry, and the newest image it drew.
//
// The app holds an array of these and blits whichever is active. A background session
// keeps running -- its pty child and pump thread never pause -- and polling it stays
// cheap because an unchanged frame comes back without drawing.

import AppKit
import CMind2tHost

final class Session {
    /// The spawn label; shown until the program sets a real title (OSC 0/2).
    let spawnTitle: String
    /// The live title, program-set. The tab strip reads this.
    private(set) var liveTitle: String?
    var title: String { liveTitle ?? spawnTitle }

    /// Deterministic work state -- explicit signals ONLY (the operator's rule: no
    /// idle guessing). OSC 9;4 progress is authoritative when a program emits it;
    /// otherwise the per-CLI title classifier reads the markers the CLI itself
    /// prints. `.done` means "was working, signal cleared, tab not yet looked at".
    enum WorkState { case idle, working, error, done }
    private(set) var workState: WorkState = .idle
    /// Which classifier table applies -- the first word of the spawned command.
    private let cli: String
    private(set) var host: OpaquePointer?
    private(set) var cols: UInt16
    private(set) var rows: UInt16
    /// Device pixels per cell, derived from the first frame -- the C surface reports
    /// pixels, not metrics.
    private(set) var cellWidth = 0
    private(set) var cellHeight = 0
    /// The newest pixels seen, kept so a switch can blit instantly without waiting for
    /// the next draw.
    private(set) var lastImage: CGImage?
    /// The terminal's own background, sampled from the frame so the window margin can
    /// wear it. When theme support arrives, the margin follows automatically -- nothing
    /// app-side hardcodes a color.
    private(set) var background: CGColor?
    /// One OSC 133 class byte per grid row (MIND2T_ROW_*), from the last drawn frame.
    /// Empty until a shell with integration has marked something. The gutter's input.
    private(set) var rowClasses: [UInt8] = []
    /// The caret's cell and visibility from the last polled frame (S4's ghost anchor).
    private(set) var cursorCol = 0
    private(set) var cursorRow = 0
    private(set) var cursorVisible = false
    /// Rows scrolled into history at last poll; ghosts only show at the live bottom.
    private(set) var viewportOffset = 0
    private(set) var exited = false

    /// The workspace (S5 worktree) this session belongs to, or nil for an ordinary
    /// session in whatever directory it opened. Purely a label plus the directory the
    /// session was PLACED in; the live cwd still comes from OSC 7, because the child is
    /// free to walk away from where it started.
    let workspace: String?

    convenience init?(
        command: String?, cols: UInt16, rows: UInt16, fontSize: Float, autoDirection: Bool,
        config: OpaquePointer? = nil, title: String, cwd: String? = nil,
        workspace: String? = nil
    ) {
        guard
            let host = spawnHost(
                cols: cols, rows: rows, fontSize: fontSize, command: command,
                autoDirection: autoDirection, config: config, cwd: cwd)
        else { return nil }
        self.init(
            host: host, cols: cols, rows: rows, title: title,
            cli: (command ?? "").split(separator: " ").first.map(String.init) ?? "shell",
            workspace: workspace)
    }

    /// Spawns an agent CLI instead of a shell, on the same pipeline.
    ///
    /// A `Result` rather than an optional, because the three ways this fails are three
    /// different things for the operator to do: install something, remove a flag, or read a
    /// real error. An optional collapses them into "nothing happened", which is how a refused
    /// bypass would look exactly like a missing binary.
    static func agent(
        _ agent: Agent, argv: [String] = [], cols: UInt16, rows: UInt16, fontSize: Float,
        autoDirection: Bool, config: OpaquePointer? = nil, cwd: String? = nil,
        workspace: String? = nil
    ) -> Result<Session, AgentLaunchFailure> {
        spawnAgentHost(
            id: agent.id, argv: argv, cols: cols, rows: rows, fontSize: fontSize,
            autoDirection: autoDirection, config: config, cwd: cwd
        )
        .map { host in
            Session(
                host: host, cols: cols, rows: rows, title: agent.name,
                // The agent's REGISTRY ID, not the binary's name. It is what the busy-marker
                // table below is keyed on, and the two differ the moment an agent ships under
                // more than one binary name -- `antigravity` answers to `agy` on a fresh
                // install, and a tab keyed on that would silently stop classifying.
                cli: agent.id, workspace: workspace)
        }
    }

    /// The one place a session's fields are set. Both spawn paths funnel here so a field
    /// added later cannot be filled in on one path and forgotten on the other.
    private init(
        host: OpaquePointer, cols: UInt16, rows: UInt16, title: String, cli: String,
        workspace: String?
    ) {
        self.host = host
        self.cols = cols
        self.rows = rows
        self.spawnTitle = title
        self.cli = cli
        self.workspace = workspace
    }

    /// Explicit-signal classification, per CLI. Claude Code marks a busy tab title
    /// with a leading spinner glyph; the table is small and honest -- a CLI with no
    /// known marker simply never classifies, and OSC 9;4 still works for it.
    private static let busyMarkers: [String: [String]] = [
        "claude": ["✳", "✶", "✻", "✽"],
    ]

    private func classify(title: String) {
        guard let markers = Session.busyMarkers[cli] else { return }
        let busy = markers.contains { title.contains($0) }
        switch (busy, workState) {
        case (true, _): workState = .working
        case (false, .working): workState = .done
        default: break
        }
    }

    func apply(progress state: UInt8) {
        switch state {
        case 0: workState = workState == .working ? .done : .idle
        case 2: workState = .error
        default: workState = .working
        }
    }

    func applyTitle(_ text: String) {
        liveTitle = text.isEmpty ? nil : text
        classify(title: text)
    }

    /// The operator looked at the tab; a done/error badge is delivered.
    func markSeen() {
        if workState == .done || workState == .error {
            workState = .idle
        }
    }

    /// Whether this session is presenting into a metal layer rather than handing back pixels.
    private(set) var presenting = false

    /// Takes over a `CAMetalLayer`, so polled frames go to the screen on the GPU.
    ///
    /// `width` and `height` are DEVICE pixels (the layer's drawableSize). Returns whether the
    /// host accepted it; a refusal means the adapter cannot drive that window or the window
    /// offered no usable format, and the caller should stay on the image path rather than show
    /// an empty layer.
    @discardableResult
    func attachLayer(_ layer: CAMetalLayer, width: Int, height: Int) -> Bool {
        guard let host, !exited else { return false }
        let raw = Unmanaged.passUnretained(layer).toOpaque()
        let ok = mind2t_host_attach_layer(host, raw, UInt32(width), UInt32(height))
            == MIND2T_HOST_SUCCESS
        presenting = ok
        return ok
    }

    func detachLayer() {
        guard let host, presenting else { return }
        _ = mind2t_host_detach_layer(host)
        presenting = false
    }

    /// Reconfigures the swapchain. DEVICE pixels.
    func resizeLayer(width: Int, height: Int) {
        guard let host, presenting else { return }
        _ = mind2t_host_resize_layer(host, UInt32(width), UInt32(height))
    }

    /// Puts the current frame on screen. No-op unless a layer is attached.
    @discardableResult
    func present() -> Bool {
        guard let host, presenting, !exited else { return false }
        return mind2t_host_present(host) == MIND2T_HOST_SUCCESS
    }

    /// Polls once. Returns a fresh image when the host drew; `exited` flips when the
    /// child is gone. Safe to call at any rate -- unchanged frames cost almost nothing.
    ///
    /// While presenting, the host leaves `frame.pixels` NULL on purpose - the frame never
    /// crosses to the CPU - so this returns nil every time and the caller drives `present()`
    /// instead of looking for an image.
    @discardableResult
    func poll() -> CGImage? {
        guard let host, !exited else { return nil }
        var frame = Mind2tHostFrame()
        guard mind2t_host_poll(host, &frame) == MIND2T_HOST_SUCCESS else { return nil }

        // The C surface reports the renderer's default background outright -- sampling
        // the corner pixel instead picks up the caret whenever the cursor sits at home,
        // which the margin then wears (measured 2026-07-29, the gray-frame screenshot).
        let (r, g, b, _) = frame.background
        background = CGColor(
            srgbRed: CGFloat(r) / 255, green: CGFloat(g) / 255, blue: CGFloat(b) / 255,
            alpha: 1)

        var fresh: CGImage?
        if frame.drew, let pixels = frame.pixels {
            fresh = Session.makeImage(pixels, width: Int(frame.width), height: Int(frame.height))
            if let fresh { lastImage = fresh }
        }
        // Derived from the REPORTED grid size, never from the pixel buffer. The C surface
        // reports width and height off the renderer's canvas whether or not it lends any
        // pixels, and while presenting it lends none by design. These two numbers gate
        // `fitToPane`, so keying them off the readback froze the grid at its spawn size for
        // the whole session: the window grew, the terminal did not, and nothing reported a
        // failure because nothing had failed. (Operator-reported from a screenshot,
        // 2026-08-04; the readback removal took the cell metrics with it.)
        if frame.drew, cellWidth == 0, frame.width > 0, frame.height > 0, cols > 0, rows > 0 {
            cellWidth = Int(frame.width) / Int(cols)
            cellHeight = Int(frame.height) / Int(rows)
        }
        if let semantics = frame.row_semantics, frame.row_count > 0 {
            rowClasses = Array(UnsafeBufferPointer(start: semantics, count: Int(frame.row_count)))
        }
        cursorCol = Int(frame.cursor_col)
        cursorRow = Int(frame.cursor_row)
        cursorVisible = frame.cursor_visible
        viewportOffset = Int(frame.viewport_offset)
        if frame.child_exited { exited = true }
        return fresh
    }

    /// One row's text, filtered by OSC 133 mark (MIND2T_TEXT_ALL for every cell). Empty
    /// when the row is out of range or nothing has been polled yet.
    func rowText(_ row: UInt16, semantic: UInt8) -> String {
        guard let host else { return "" }
        var length = 0
        guard mind2t_host_row_text(host, row, semantic, nil, 0, &length) == MIND2T_HOST_SUCCESS,
            length > 0
        else { return "" }
        var buffer = [UInt8](repeating: 0, count: length)
        guard
            buffer.withUnsafeMutableBufferPointer({ pointer in
                mind2t_host_row_text(host, row, semantic, pointer.baseAddress, length, &length)
            }) == MIND2T_HOST_SUCCESS
        else { return "" }
        return String(decoding: buffer, as: UTF8.self)
    }

    enum HostEvent {
        case clipboard(String)
        case notify(title: String, body: String)
        case bell
        case title(String)
        case progress(state: UInt8)
        /// OSC 133;C: execution began -- the typed command is final (S4 records it).
        case commandStart
        /// OSC 7: the working directory, RAW and undecoded. Kept as bytes because the
        /// host normalizes it (percent-escapes, the file:// host) and doing it twice in
        /// two languages is how the two copies drift apart.
        case pwd([UInt8])
    }

    /// The last OSC 7 report this session made, raw. History is keyed by it.
    private(set) var pwdRaw: [UInt8] = []

    /// Records a command under the directory this session last reported.
    ///
    /// Both history calls live here rather than inline at the view layer because
    /// `pwdRaw` is the argument that makes them correct, and getting it wrong is
    /// SILENT: an empty cwd falls back to global history and looks exactly like the
    /// feature not existing. Keeping them on the session is what lets
    /// `--smoke-history` drive the same code the window runs, instead of a copy of it.
    func recordCommand(_ store: OpaquePointer?, command: String) {
        let bytes = Array(command.utf8)
        _ = bytes.withUnsafeBufferPointer { pointer in
            pwdRaw.withUnsafeBufferPointer { cwdPointer in
                mind2t_history_append(
                    store, pointer.baseAddress, pointer.count,
                    cwdPointer.baseAddress, pwdRaw.count)
            }
        }
    }

    /// The whole suggested command for `input` (not the remainder), or nil when the
    /// store has nothing. A match made in this directory outranks a newer one made
    /// elsewhere -- the preference is the host's, and `pwdRaw` is how it learns where
    /// "here" is.
    func suggestion(_ store: OpaquePointer?, for input: String) -> String? {
        let inputBytes = Array(input.utf8)
        var length = 0
        let sized = inputBytes.withUnsafeBufferPointer { pointer in
            pwdRaw.withUnsafeBufferPointer { cwdPointer in
                mind2t_history_suggest(
                    store, pointer.baseAddress, pointer.count,
                    cwdPointer.baseAddress, pwdRaw.count, nil, 0, &length)
            }
        }
        guard sized == MIND2T_HOST_SUCCESS, length > 0 else { return nil }

        var buffer = [UInt8](repeating: 0, count: length)
        let copied = inputBytes.withUnsafeBufferPointer { pointer in
            pwdRaw.withUnsafeBufferPointer { cwdPointer in
                buffer.withUnsafeMutableBufferPointer { outPointer in
                    mind2t_history_suggest(
                        store, pointer.baseAddress, pointer.count,
                        cwdPointer.baseAddress, pwdRaw.count,
                        outPointer.baseAddress, length, &length)
                }
            }
        }
        guard copied == MIND2T_HOST_SUCCESS else { return nil }
        return String(decoding: buffer, as: UTF8.self)
    }

    /// Drains every pending host-facing event, oldest first. The C contract consumes an
    /// event only when the buffer held it, so size-then-fetch loses nothing.
    func drainEvents() -> [HostEvent] {
        guard let host else { return [] }
        var drained: [HostEvent] = []
        while true {
            var kind: UInt32 = 0
            var length = 0
            guard mind2t_host_next_event(host, &kind, nil, 0, &length) == MIND2T_HOST_SUCCESS,
                kind != 0
            else { break }
            var payload = [UInt8](repeating: 0, count: length)
            if length > 0 {
                var fetched: UInt32 = 0
                guard
                    payload.withUnsafeMutableBufferPointer({ pointer in
                        mind2t_host_next_event(
                            host, &fetched, pointer.baseAddress, length, &length)
                    }) == MIND2T_HOST_SUCCESS, fetched == kind
                else { break }
            }
            let text = String(decoding: payload, as: UTF8.self)
            switch kind {
            case 1: drained.append(.clipboard(text))
            case 2:
                let parts = text.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false)
                drained.append(
                    .notify(
                        title: parts.first.map(String.init) ?? "",
                        body: parts.count > 1 ? String(parts[1]) : ""))
            case 3: drained.append(.bell)
            case 4: drained.append(.title(text))
            case 5: drained.append(.progress(state: payload.first ?? 0))
            case 6: drained.append(.commandStart)
            case 7:
                // Held on the session as well as delivered: a suggestion is asked for on
                // ticks where no event arrived, and it still needs to know where it is.
                pwdRaw = payload
                drained.append(.pwd(payload))
            default: break
            }
        }
        return drained
    }

    /// The OSC 8 URI under a cell, if any (last polled frame).
    func linkAt(col: UInt16, row: UInt16) -> String? {
        guard let host else { return nil }
        var length = 0
        guard mind2t_host_link_at(host, col, row, nil, 0, &length) == MIND2T_HOST_SUCCESS,
            length > 0
        else { return nil }
        var buffer = [UInt8](repeating: 0, count: length)
        guard
            buffer.withUnsafeMutableBufferPointer({ pointer in
                mind2t_host_link_at(host, col, row, pointer.baseAddress, length, &length)
            }) == MIND2T_HOST_SUCCESS
        else { return nil }
        return String(decoding: buffer, as: UTF8.self)
    }

    /// Scrolls the view through scrollback; positive climbs into history, negative
    /// returns, `Int32.min` snaps to the live bottom (the host's documented sentinel).
    /// The host clamps against what history holds; the landed position rides back in
    /// the next polled frame's viewport_offset.
    func scroll(_ rows: Int32) {
        guard let host else { return }
        _ = mind2t_host_scroll(host, rows)
    }

    func send(_ bytes: [UInt8]) {
        guard let host else { return }
        _ = bytes.withUnsafeBufferPointer { buffer in
            mind2t_host_send(host, buffer.baseAddress, buffer.count)
        }
    }

    /// Tells the host what pixel space pointer events arrive in: the view's size and
    /// content insets, in backing pixels. Re-sent on every layout change AND on session
    /// activation -- geometry is per-host state, and a session spawned while another
    /// was frontmost has never seen the view.
    func mouseGeometry(width: UInt32, height: UInt32, inset: UInt32) {
        guard let host else { return }
        _ = mind2t_host_mouse_geometry(host, width, height, inset, inset, inset, inset)
    }

    /// One pointer event in surface pixels. True when the terminal consumed it (a
    /// report went to the child); false hands the event back to AppKit.
    func mouse(action: UInt32, button: UInt32, mods: UInt32, x: Float, y: Float) -> Bool {
        guard let host else { return false }
        return mind2t_host_mouse(host, action, button, mods, x, y) == MIND2T_HOST_SUCCESS
    }

    /// One wheel gesture in whole ticks, positive up. True when the terminal consumed
    /// it (mouse-mode report or alternate-scroll arrows); false means the viewport
    /// scroll is ours.
    func wheel(x: Float, y: Float, ticks: Int32, mods: UInt32) -> Bool {
        guard let host else { return false }
        return mind2t_host_wheel(host, x, y, ticks, mods) == MIND2T_HOST_SUCCESS
    }

    /// One keyboard event through the host's encoder -- every mode (DECCKM, keypad,
    /// kitty flags) rides the frame, so this replaces byte-building entirely. True
    /// when the terminal produced bytes.
    func key(
        action: UInt32, key: UInt32, mods: UInt32, consumedMods: UInt32,
        text: [UInt8], unshiftedCodepoint: UInt32
    ) -> Bool {
        guard let host else { return false }
        return text.withUnsafeBufferPointer { buffer in
            mind2t_host_key(
                host, action, key, mods, consumedMods,
                buffer.baseAddress, buffer.count, unshiftedCodepoint)
        } == MIND2T_HOST_SUCCESS
    }

    func paste(_ bytes: [UInt8]) {
        guard let host else { return }
        _ = bytes.withUnsafeBufferPointer { buffer in
            mind2t_host_paste(host, buffer.baseAddress, buffer.count)
        }
    }

    func resize(cols: UInt16, rows: UInt16) {
        guard let host, cols != self.cols || rows != self.rows else { return }
        if mind2t_host_resize(host, cols, rows) == MIND2T_HOST_SUCCESS {
            self.cols = cols
            self.rows = rows
        }
    }

    /// Live zoom: pty resize plus renderer rebuild at the new size, one C call. Cell
    /// metrics re-derive from the next frame -- the C surface reports pixels, and the
    /// old numbers describe a dead font.
    func setFontSize(_ fontSize: Float, cols: UInt16, rows: UInt16) {
        guard let host else { return }
        if mind2t_host_set_font_size(host, fontSize, cols, rows) == MIND2T_HOST_SUCCESS {
            self.cols = cols
            self.rows = rows
            cellWidth = 0
            cellHeight = 0
        }
    }

    func close() {
        if let host {
            mind2t_host_free(host)
            self.host = nil
        }
    }

    deinit { close() }

    private static func makeImage(
        _ pixels: UnsafePointer<UInt8>, width: Int, height: Int
    ) -> CGImage? {
        // Copied because the borrow dies at the next poll, and CoreGraphics reads lazily.
        let data = Data(bytes: pixels, count: width * height * 4)
        guard let provider = CGDataProvider(data: data as CFData) else { return nil }
        return CGImage(
            width: width,
            height: height,
            bitsPerComponent: 8,
            bitsPerPixel: 32,
            bytesPerRow: width * 4,
            // The core's palette bytes are sRGB values and the render crate deliberately
            // does no gamma work ("any gamma decision belongs to whoever puts these
            // pixels on a screen" -- gpu.rs). DeviceRGB would hand those numbers to the
            // panel's native gamut, oversaturating every colour on a P3 display.
            space: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue),
            provider: provider,
            decode: nil,
            shouldInterpolate: false,
            intent: .defaultIntent
        )
    }
}

extension Optional where Wrapped == String {
    /// Runs `body` with a C string for the wrapped value, or NULL for nil -- the shape
    /// every optional-string C parameter here wants.
    func withCStringOrNil<R>(_ body: (UnsafePointer<CChar>?) -> R) -> R {
        switch self {
        case .some(let value): return value.withCString { body($0) }
        case .none: return body(nil)
        }
    }
}
