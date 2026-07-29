// The terminal surface: blit target, key forwarding, and the app's keyboard chords.
//
// The view draws exactly what ruuah_host_poll hands over -- an RGBA8 buffer at native
// (backing) resolution, because the host is spawned with font_size scaled by the window's
// backingScaleFactor and the layer's contentsScale set to match. Pixels map 1:1 to device
// pixels; nothing is stretched. Session plumbing lives in AppDelegate.swift.

import AppKit

final class TerminalView: NSView {
    /// Breathing room between the glyph grid and the window edge, in points. The grid
    /// itself stays exactly cols x rows; the margin belongs to the window, not the pty.
    static let padding: CGFloat = 8

    /// The terminal image draws on this sublayer, inset by `padding`. The root layer
    /// holds only the background color, so the margin matches the terminal's own black.
    let contentLayer = CALayer()

    var onKeyBytes: (([UInt8]) -> Void)?
    var onPaste: (([UInt8]) -> Void)?
    var onNewSession: (() -> Void)?
    var onCloseSession: (() -> Void)?
    var onZoomIn: (() -> Void)?
    var onZoomOut: (() -> Void)?
    var onZoomReset: (() -> Void)?
    /// A click on a block's gutter bar. The receiver owns the menu and the actions.
    var onBlockClick: ((Block, NSEvent) -> Void)?

    /// The gutter (S2): one thin bar per block in the left margin, drawn from the
    /// shell's OSC 133 marks. Empty without shell integration -- the margin stays bare
    /// rather than guessing at boundaries.
    private var blocks: [Block] = []
    private var bars: [CALayer] = []
    /// Device pixels per cell row, from the active session's first frame; 0 = no grid.
    private var cellHeightDevice = 0
    private static let barColor = NSColor(srgbRed: 1, green: 1, blue: 1, alpha: 0.22).cgColor
    private static let barHotColor =
        NSColor(srgbRed: 0x58 / 255.0, green: 0x65 / 255.0, blue: 0xF2 / 255.0, alpha: 1).cgColor

    override var acceptsFirstResponder: Bool { true }

    override func layout() {
        super.layout()
        contentLayer.frame = bounds.insetBy(
            dx: TerminalView.padding, dy: TerminalView.padding)
        layoutBars()
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

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        guard let block = block(at: point) else {
            super.mouseDown(with: event)
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
        let mods = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        // The zoom pair also answers with shift held: cmd+shift+= is how most fingers
        // type "cmd plus", and nothing else owns those chords.
        let zoomKey = event.keyCode == 24 || event.keyCode == 27
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
        case 13:  // kVK_ANSI_W
            onCloseSession?()
            return true
        default:
            return super.performKeyEquivalent(with: event)
        }
    }

    override func keyDown(with event: NSEvent) {
        guard let bytes = encode(event) else { return }
        onKeyBytes?(bytes)
    }

    /// The minimal key encoder: printables and control characters pass through as the
    /// UTF-8 AppKit already produced; arrows become their CSI sequences. This lives in the
    /// GUI on purpose -- outside an input region the cursor is the running program's to
    /// place, so the core never encodes keys (slice 5.6's rule).
    private func encode(_ event: NSEvent) -> [UInt8]? {
        if let special = event.specialKey {
            switch special {
            case .upArrow: return [0x1b, 0x5b, 0x41]
            case .downArrow: return [0x1b, 0x5b, 0x42]
            case .rightArrow: return [0x1b, 0x5b, 0x43]
            case .leftArrow: return [0x1b, 0x5b, 0x44]
            case .delete, .backspace, .deleteForward: return [0x7f]
            case .carriageReturn, .enter: return [0x0d]
            case .tab: return [0x09]
            default: break
            }
        }
        guard let characters = event.characters, !characters.isEmpty else { return nil }
        // Function-key scalars (U+F700...) are AppKit's own encoding, not bytes a child
        // understands; anything not handled above is dropped rather than leaked.
        if characters.unicodeScalars.contains(where: { $0.value >= 0xF700 && $0.value <= 0xF8FF }) {
            return nil
        }
        return Array(characters.utf8)
    }
}
