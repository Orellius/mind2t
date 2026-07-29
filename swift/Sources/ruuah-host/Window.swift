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

    override var acceptsFirstResponder: Bool { true }

    override func layout() {
        super.layout()
        contentLayer.frame = bounds.insetBy(
            dx: TerminalView.padding, dy: TerminalView.padding)
    }

    /// The app's chords, caught before the key-equivalent machinery falls through to
    /// keyDown -- the minimal host has no menu bar to own them. cmd+V hands the host raw
    /// clipboard bytes: the host owns the encoding (fenceposts or newline folding, by the
    /// child's mode 2004), and building either sequence here would duplicate the
    /// oracle-measured transform.
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let commandOnly: NSEvent.ModifierFlags = [.command]
        guard event.modifierFlags.intersection(.deviceIndependentFlagsMask) == commandOnly
        else { return super.performKeyEquivalent(with: event) }
        switch event.charactersIgnoringModifiers {
        case "v":
            if let text = NSPasteboard.general.string(forType: .string) {
                onPaste?(Array(text.utf8))
                return true
            }
            return false
        case "t":
            onNewSession?()
            return true
        case "w":
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
