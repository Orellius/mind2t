// The terminal surface: blit target, key forwarding, and the app's keyboard chords.
//
// The view draws exactly what ruuah_host_poll hands over -- an RGBA8 buffer at native
// (backing) resolution, because the host is spawned with font_size scaled by the window's
// backingScaleFactor and the layer's contentsScale set to match. Pixels map 1:1 to device
// pixels; nothing is stretched. Session plumbing lives in AppDelegate.swift.

import AppKit

/// Where the chrome's three pieces go, as arithmetic with no views in it.
///
/// Extracted from `layoutChrome` because docking is the one part of S5.5 with a real
/// failure mode, and it is a SILENT one: a pane that keeps its full width while a
/// sidebar is docked simply draws underneath it, and the terminal looks fine until you
/// notice the right-hand columns are covered. The invariant worth asserting is that the
/// pane and the sidebar TILE the content exactly -- no overlap, no gap -- and that is
/// only assertable if the arithmetic exists apart from the NSViews.
struct ChromeLayout: Equatable {
    let tabBar: NSRect
    let pane: NSRect
    /// nil when the sidebar is not docked.
    let sidebar: NSRect?

    /// The narrowest the terminal pane may become; past this the sidebar is what gives.
    static let minimumPaneWidth: CGFloat = 120

    static func compute(
        content: NSSize, tabHeight: CGFloat, sidebarWidth: CGFloat?
    ) -> ChromeLayout {
        let below = max(0, content.height - tabHeight)
        let tabBar = NSRect(
            x: 0, y: below, width: content.width, height: min(tabHeight, content.height))

        guard let requested = sidebarWidth else {
            return ChromeLayout(
                tabBar: tabBar,
                pane: NSRect(x: 0, y: 0, width: content.width, height: below),
                sidebar: nil)
        }
        // The pane's floor wins over the sidebar's preferred width, and the sidebar
        // takes whatever is left. Computing the sidebar as a REMAINDER rather than as a
        // constant is what keeps the two tiling exactly on a narrow window, where
        // subtracting a fixed 300 would leave them overlapping.
        let paneWidth = max(minimumPaneWidth, content.width - requested)
        let clampedPane = min(paneWidth, content.width)
        return ChromeLayout(
            tabBar: tabBar,
            pane: NSRect(x: 0, y: 0, width: clampedPane, height: below),
            sidebar: NSRect(
                x: clampedPane, y: 0, width: content.width - clampedPane, height: below))
    }
}

final class TerminalView: NSView {
    /// Breathing room between the glyph grid and the window edge, in points. The grid
    /// itself stays exactly cols x rows; the margin belongs to the window, not the pty.
    static let padding: CGFloat = 8

    /// The terminal image draws on this sublayer, inset by `padding`. The root layer
    /// holds only the background color, so the margin matches the terminal's own black.
    let contentLayer = CALayer()

    /// One keyboard event for the host's key encoder: (action, key, mods,
    /// consumedMods, utf8, unshiftedCodepoint) in the ruuah_host_key vocabulary.
    /// Returns whether the terminal produced bytes.
    var onKey: ((UInt32, UInt32, UInt32, UInt32, [UInt8], UInt32) -> Bool)?
    var onPaste: (([UInt8]) -> Void)?
    var onNewSession: (() -> Void)?
    /// cmd+K: the receiver presents (or toggles) the command palette.
    var onPalette: (() -> Void)?
    /// cmd+shift+D: the receiver toggles the diff review panel (S6). Unset, and so a
    /// dead chord, unless `panels = true` in the config.
    var onDiffPanel: (() -> Void)?
    /// A bare right-arrow while a ghost suggestion shows: returns whether the
    /// receiver accepted it (typed the remainder); false lets the key go to the child.
    var onAcceptSuggestion: (() -> Bool)?
    var onCloseSession: (() -> Void)?
    var onZoomIn: (() -> Void)?
    var onZoomOut: (() -> Void)?
    var onZoomReset: (() -> Void)?
    /// A click on a block's gutter bar. The receiver owns the menu and the actions.
    var onBlockClick: ((Block, NSEvent) -> Void)?
    /// cmd+click anywhere on the grid; the receiver resolves the cell and its OSC 8
    /// link, because only it knows the active session's cell metrics.
    var onCommandClick: ((NSPoint) -> Void)?
    /// Scrollback rows: positive climbs into history, Int32.min snaps to the bottom.
    /// The receiver forwards to the active session's host, which owns the clamping.
    var onScroll: ((Int32) -> Void)?
    /// One pointer event for mouse reporting: (action, button, mods, x, y) in the
    /// RUUAH_MOUSE_* vocabulary and surface pixels. Returns whether the terminal
    /// consumed it; false hands the event back to AppKit's default handling.
    var onMouse: ((UInt32, UInt32, UInt32, Float, Float) -> Bool)?
    /// One wheel gesture: (x, y, whole ticks positive-up, mods). Returns whether the
    /// terminal consumed it (mouse-mode report or alternate-scroll arrows); false
    /// means the viewport scroll below is ours.
    var onWheel: ((Float, Float, Int32, UInt32) -> Bool)?
    /// The view's pixel geometry for the mouse encoder: (width, height, inset), all in
    /// backing pixels. Fired from layout and backing-scale changes.
    var onMouseGeometry: ((UInt32, UInt32, UInt32) -> Void)?

    /// The gutter (S2): one thin bar per block in the left margin, drawn from the
    /// shell's OSC 133 marks. Empty without shell integration -- the margin stays bare
    /// rather than guessing at boundaries.
    private var blocks: [Block] = []
    private var bars: [CALayer] = []
    /// Device pixels per cell row, from the active session's first frame; 0 = no grid.
    private var cellHeightDevice = 0
    /// Fractional wheel rows banked between events, so a slow trackpad swipe still
    /// eventually moves a row instead of being rounded away on every tick.
    private var wheelRemainder: CGFloat = 0
    private static let barColor = NSColor(srgbRed: 1, green: 1, blue: 1, alpha: 0.22).cgColor
    private static let barHotColor =
        NSColor(srgbRed: 0x58 / 255.0, green: 0x65 / 255.0, blue: 0xF2 / 255.0, alpha: 1).cgColor

    override var acceptsFirstResponder: Bool { true }

    override func layout() {
        super.layout()
        contentLayer.frame = bounds.insetBy(
            dx: TerminalView.padding, dy: TerminalView.padding)
        layoutBars()
        pushMouseGeometry()
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        pushMouseGeometry()
    }

    /// Sends the current pixel geometry to whoever owns the hosts. Public because
    /// geometry is PER-HOST state: session activation must re-send it to a host that
    /// was spawned or backgrounded while another session owned the view.
    func pushMouseGeometry() {
        guard let scale = window?.backingScaleFactor, bounds.width > 0, bounds.height > 0
        else { return }
        onMouseGeometry?(
            UInt32((bounds.width * scale).rounded()),
            UInt32((bounds.height * scale).rounded()),
            UInt32((TerminalView.padding * scale).rounded()))
    }

    /// A pointer location as the mouse encoder's surface pixels: view top-left origin
    /// (AppKit grows upward, the grid downward), backing scale applied.
    private func surfacePoint(_ event: NSEvent) -> (Float, Float)? {
        guard let scale = window?.backingScaleFactor else { return nil }
        let point = convert(event.locationInWindow, from: nil)
        return (Float(point.x * scale), Float((bounds.height - point.y) * scale))
    }

    private static func mouseMods(_ flags: NSEvent.ModifierFlags) -> UInt32 {
        var mods: UInt32 = 0
        if flags.contains(.shift) { mods |= 1 }
        if flags.contains(.control) { mods |= 2 }
        if flags.contains(.option) { mods |= 4 }
        return mods
    }

    /// NSEvent.buttonNumber to the protocol's codes, Ghostty's own macOS mapping:
    /// 3/4 are back/forward (protocol 8/9), 7/8 the rare physical 4/5.
    private static func protocolButton(_ number: Int) -> UInt32 {
        switch number {
        case 0: return 1  // left
        case 1: return 3  // right
        case 2: return 2  // middle
        case 3: return 8
        case 4: return 9
        case 5: return 6
        case 6: return 7
        case 7: return 4
        case 8: return 5
        default: return 10  // unnamed in the protocol; bookkeeping only
        }
    }

    /// Forwards one pointer event to mouse reporting. Motion without any pressed
    /// button reports button "none" -- buttonNumber is meaningless on pure moves.
    @discardableResult
    private func forwardMouse(_ event: NSEvent, action: UInt32) -> Bool {
        guard let (x, y) = surfacePoint(event) else { return false }
        let button: UInt32 =
            event.type == .mouseMoved ? 0 : TerminalView.protocolButton(event.buttonNumber)
        return onMouse?(action, button, TerminalView.mouseMods(event.modifierFlags), x, y)
            ?? false
    }

    /// Motion tracking for mode 1003 (and drag-outside delivery). Installed always:
    /// with reporting off the host answers Ignored immediately, and the dedup state
    /// keeps the traffic bounded while it is on.
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for area in trackingAreas {
            removeTrackingArea(area)
        }
        addTrackingArea(
            NSTrackingArea(
                rect: .zero,
                options: [.mouseMoved, .activeInKeyWindow, .inVisibleRect],
                owner: self))
    }

    /// Replaces the gutter with the active session's blocks. Cheap when nothing moved.
    func updateGutter(blocks: [Block], cellHeightDevice: Int) {
        guard blocks != self.blocks || cellHeightDevice != self.cellHeightDevice else { return }
        self.blocks = blocks
        self.cellHeightDevice = cellHeightDevice
        while bars.count > blocks.count {
            bars.removeLast().removeFromSuperlayer()
        }
        while bars.count < blocks.count {
            let bar = CALayer()
            bar.actions = ["bounds": NSNull(), "position": NSNull(), "backgroundColor": NSNull()]
            bar.cornerRadius = 1
            layer?.addSublayer(bar)
            bars.append(bar)
        }
        layoutBars()
    }

    /// One point-space rect per block bar, y-flipped (AppKit views grow upward; grid
    /// rows grow downward from the top padding edge).
    private func layoutBars() {
        guard cellHeightDevice > 0, let scale = window?.backingScaleFactor else { return }
        let rowHeight = CGFloat(cellHeightDevice) / scale
        for (block, bar) in zip(blocks, bars) {
            let top = TerminalView.padding + CGFloat(block.rows.lowerBound) * rowHeight
            let height = CGFloat(block.rows.count) * rowHeight
            bar.frame = CGRect(
                x: 2, y: bounds.height - top - height, width: 3, height: height - 2)
            bar.backgroundColor = TerminalView.barColor
        }
    }

    /// The block whose bar band contains a click in the left margin, if any.
    private func block(at point: NSPoint) -> Block? {
        guard point.x < TerminalView.padding, cellHeightDevice > 0,
            let scale = window?.backingScaleFactor
        else { return nil }
        let rowHeight = CGFloat(cellHeightDevice) / scale
        let row = Int((bounds.height - point.y - TerminalView.padding) / rowHeight)
        return blocks.first { $0.rows.contains(row) }
    }

    /// One visible page of rows, for the cmd+PageUp/PageDown chords. Derived from the
    /// content layer and the live cell metrics rather than asked of anyone.
    private func pageRows() -> Int32 {
        guard cellHeightDevice > 0, let scale = window?.backingScaleFactor else { return 0 }
        return Int32(max(1, Int(contentLayer.bounds.height * scale) / cellHeightDevice))
    }

    /// Trackpad and wheel scrolling drive the scrollback viewport. Positive deltaY --
    /// fingers or wheel moving the content down, toward earlier output -- climbs into
    /// history, the direction every scrolling view on the platform means by it. Precise
    /// deltas arrive in points and are banked as fractional rows; line-based wheels
    /// report rows directly.
    override func scrollWheel(with event: NSEvent) {
        guard cellHeightDevice > 0, let scale = window?.backingScaleFactor else { return }
        let rowHeight = CGFloat(cellHeightDevice) / scale
        wheelRemainder +=
            event.hasPreciseScrollingDeltas
            ? event.scrollingDeltaY / rowHeight
            : event.scrollingDeltaY
        let rows = Int32(wheelRemainder.rounded(.towardZero))
        if rows != 0 {
            wheelRemainder -= CGFloat(rows)
            // The terminal's precedence first (mouse-mode wheel reports, alternate
            // scroll); the viewport is the fallback, exactly the host's contract.
            if let (x, y) = surfacePoint(event),
                onWheel?(x, y, rows, TerminalView.mouseMods(event.modifierFlags)) == true
            {
                return
            }
            onScroll?(rows)
        }
    }

    override func mouseUp(with event: NSEvent) {
        if !forwardMouse(event, action: 1) {
            super.mouseUp(with: event)
        }
    }

    override func mouseDragged(with event: NSEvent) {
        if !forwardMouse(event, action: 2) {
            super.mouseDragged(with: event)
        }
    }

    override func mouseMoved(with event: NSEvent) {
        if !forwardMouse(event, action: 2) {
            super.mouseMoved(with: event)
        }
    }

    override func rightMouseDown(with event: NSEvent) {
        if !forwardMouse(event, action: 0) {
            super.rightMouseDown(with: event)
        }
    }

    override func rightMouseUp(with event: NSEvent) {
        if !forwardMouse(event, action: 1) {
            super.rightMouseUp(with: event)
        }
    }

    override func rightMouseDragged(with event: NSEvent) {
        if !forwardMouse(event, action: 2) {
            super.rightMouseDragged(with: event)
        }
    }

    override func otherMouseDown(with event: NSEvent) {
        if !forwardMouse(event, action: 0) {
            super.otherMouseDown(with: event)
        }
    }

    override func otherMouseUp(with event: NSEvent) {
        if !forwardMouse(event, action: 1) {
            super.otherMouseUp(with: event)
        }
    }

    override func otherMouseDragged(with event: NSEvent) {
        if !forwardMouse(event, action: 2) {
            super.otherMouseDragged(with: event)
        }
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if event.modifierFlags.contains(.command) {
            onCommandClick?(point)
            return
        }
        guard let block = block(at: point) else {
            // The app's own affordances (cmd+click links, the gutter) outrank
            // reporting; past them the child's mouse protocol gets the click.
            if !forwardMouse(event, action: 0) {
                super.mouseDown(with: event)
            }
            return
        }
        // The clicked bar flashes hot so the menu visibly belongs to it.
        if let index = blocks.firstIndex(of: block), index < bars.count {
            bars[index].backgroundColor = TerminalView.barHotColor
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { [weak self] in
                guard let self, index < self.bars.count else { return }
                self.bars[index].backgroundColor = TerminalView.barColor
            }
        }
        onBlockClick?(block, event)
    }

    /// The app's chords, caught before the key-equivalent machinery falls through to
    /// keyDown -- the minimal host has no menu bar to own them. cmd+V hands the host raw
    /// clipboard bytes: the host owns the encoding (fenceposts or newline folding, by the
    /// child's mode 2004), and building either sequence here would duplicate the
    /// oracle-measured transform.
    ///
    /// Matched on the PHYSICAL key, never on characters: under a Hebrew input source the
    /// V key reports "ו" in charactersIgnoringModifiers, and a character match silently
    /// disables every chord for exactly the user this terminal is built for (found live,
    /// 2026-07-29). ANSI key codes are layout-independent.
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        // Function keys (PageUp/PageDown/Home/End) stamp .function and .numericPad into
        // the flags on their own; stripped so "cmd alone" still reads as cmd alone.
        let mods = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
            .subtracting([.function, .numericPad])
        // The zoom pair also answers with shift held: cmd+shift+= is how most fingers
        // type "cmd plus", and nothing else owns those chords.
        let zoomKey = event.keyCode == 24 || event.keyCode == 27
        // cmd+shift+D is the panel. Shift-qualified deliberately: bare cmd+D belongs to
        // the child (it is EOF-adjacent muscle memory) and taking it would be a chord
        // the terminal stole from every program running in it.
        if mods == [.command, .shift], event.keyCode == 2, let onDiffPanel {
            onDiffPanel()
            return true
        }
        guard mods == [.command] || (mods == [.command, .shift] && zoomKey)
        else { return super.performKeyEquivalent(with: event) }
        switch event.keyCode {
        case 24:  // kVK_ANSI_Equal -- cmd+= / cmd++
            onZoomIn?()
            return true
        case 27:  // kVK_ANSI_Minus -- cmd+-
            onZoomOut?()
            return true
        case 29:  // kVK_ANSI_0 -- cmd+0, back to the configured size
            onZoomReset?()
            return true
        case 9:  // kVK_ANSI_V
            if let text = NSPasteboard.general.string(forType: .string) {
                onPaste?(Array(text.utf8))
                return true
            }
            return false
        case 17:  // kVK_ANSI_T
            onNewSession?()
            return true
        case 40:  // kVK_ANSI_K -- cmd+K, the command palette
            onPalette?()
            return true
        case 13:  // kVK_ANSI_W
            onCloseSession?()
            return true
        case 116:  // kVK_PageUp -- cmd+page-up, one page into history
            onScroll?(pageRows())
            return true
        case 121:  // kVK_PageDown -- cmd+page-down, one page back toward the bottom
            onScroll?(-pageRows())
            return true
        case 115:  // kVK_Home -- cmd+home, the top of scrollback (host clamps)
            onScroll?(Int32.max / 2)
            return true
        case 119:  // kVK_End -- cmd+end, the live bottom
            onScroll?(Int32.min)
            return true
        default:
            return super.performKeyEquivalent(with: event)
        }
    }

    override func keyDown(with event: NSEvent) {
        // Fish's accept gesture: a BARE right-arrow while a ghost shows completes it.
        // Arrow keys stamp .function/.numericPad on their own, so those don't count
        // as modifiers; any real modifier lets the arrow reach the child untouched.
        if event.keyCode == 124,
            event.modifierFlags.intersection(.deviceIndependentFlagsMask)
                .subtracting([.function, .numericPad]).isEmpty,
            onAcceptSuggestion?() == true
        {
            return
        }
        forwardKey(event, action: event.isARepeat ? 2 : 1)
    }

    /// Releases matter now: kitty's report-events flag asks for them, and the host
    /// answers Ignored under every legacy mode, so forwarding is free.
    override func keyUp(with event: NSEvent) {
        forwardKey(event, action: 0)
    }

    /// A modifier's own press and release arrive as flagsChanged, not keyDown; the
    /// keyCode names the modifier and its flag's presence gives the direction. Kitty
    /// mode with report-all is the consumer. NEVER read characters here -- AppKit
    /// raises on flagsChanged events -- hence the empty text override.
    override func flagsChanged(with event: NSEvent) {
        guard let flag = TerminalView.modifierFlag(for: event.keyCode) else {
            super.flagsChanged(with: event)
            return
        }
        let action: UInt32 = event.modifierFlags.contains(flag) ? 1 : 0
        forwardKey(event, action: action, textOverride: [])
    }

    /// Which modifier a flagsChanged keyCode names (left/right pairs share a flag).
    private static func modifierFlag(for keyCode: UInt16) -> NSEvent.ModifierFlags? {
        switch keyCode {
        case 56, 60: return .shift
        case 59, 62: return .control
        case 58, 61: return .option
        case 54, 55: return .command
        case 57: return .capsLock
        default: return nil
        }
    }

    /// NSEvent modifiers to the GhosttyMods bitmask the encoder speaks. macOS has no
    /// numlock flag; the bit stays clear and mode 1035's default covers the keypad.
    private static func keyMods(_ flags: NSEvent.ModifierFlags) -> UInt32 {
        var mods: UInt32 = 0
        if flags.contains(.shift) { mods |= 1 }
        if flags.contains(.control) { mods |= 2 }
        if flags.contains(.option) { mods |= 4 }
        if flags.contains(.command) { mods |= 8 }
        if flags.contains(.capsLock) { mods |= 16 }
        return mods
    }

    /// The event's translated text, Ghostty's own macOS rules (NSEvent+Extension.swift):
    /// a lone control character is re-translated without control -- the encoder owns
    /// control mapping -- and AppKit's private-use function-key scalars (U+F700...)
    /// never reach the child. Releases carry no text; kitty's associated text is a
    /// press/repeat concept.
    private static func eventText(_ event: NSEvent) -> [UInt8] {
        guard event.type == .keyDown, let characters = event.characters, !characters.isEmpty
        else { return [] }
        if characters.count == 1, let scalar = characters.unicodeScalars.first,
            scalar.value < 0x20
        {
            let plain = event.characters(
                byApplyingModifiers: event.modifierFlags.subtracting(.control))
            return Array((plain ?? "").utf8)
        }
        if characters.unicodeScalars.contains(where: { $0.value >= 0xF700 && $0.value <= 0xF8FF })
        {
            return []
        }
        return Array(characters.utf8)
    }

    /// Builds the host key event: the generated keycode map for identity, mods minus
    /// control and command as consumed (Ghostty's years-old heuristic -- those two
    /// never contribute to text translation on macOS), and the unmodified codepoint
    /// via byApplyingModifiers rather than charactersIgnoringModifiers, whose
    /// behavior shifts under ctrl.
    @discardableResult
    private func forwardKey(
        _ event: NSEvent, action: UInt32, textOverride: [UInt8]? = nil
    ) -> Bool {
        let key = KeyMap.keyByCode[event.keyCode] ?? 0
        let mods = TerminalView.keyMods(event.modifierFlags)
        let consumed = TerminalView.keyMods(
            event.modifierFlags.subtracting([.control, .command]))
        let text = textOverride ?? TerminalView.eventText(event)
        var unshifted: UInt32 = 0
        if event.type == .keyDown || event.type == .keyUp,
            let chars = event.characters(byApplyingModifiers: []),
            let scalar = chars.unicodeScalars.first
        {
            unshifted = scalar.value
        }
        return onKey?(action, key, mods, consumed, text, unshifted) ?? false
    }
}
