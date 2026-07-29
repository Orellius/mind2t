// The window half of the minimal host: blit polled frames, forward keys, live resize.
//
// The view draws exactly what ruuah_host_poll hands over -- an RGBA8 buffer at native
// (backing) resolution, because the host is spawned with font_size scaled by the window's
// backingScaleFactor and the layer's contentsScale set to match. Pixels map 1:1 to device
// pixels; nothing is stretched.

import AppKit
import CRuuahHost

final class TerminalView: NSView {
    var onKeyBytes: (([UInt8]) -> Void)?

    override var acceptsFirstResponder: Bool { true }

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

final class HostAppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    private let command: String?
    private var host: OpaquePointer?
    private var window: NSWindow!
    private var view: TerminalView!
    private var timer: Timer?
    private var cols: UInt16 = 80
    private var rows: UInt16 = 24
    private var cellWidth = 0
    private var cellHeight = 0

    init(command: String?) {
        self.command = command
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        view = TerminalView(frame: .zero)
        view.wantsLayer = true

        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 800, height: 480),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "ruuah-vt host"
        window.contentView = view
        window.delegate = self
        window.center()
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(view)
        NSApp.activate(ignoringOtherApps: true)

        // Native resolution: the renderer rasterizes at backing scale, the layer declares
        // it, and one buffer pixel is one device pixel.
        let scale = Float(window.backingScaleFactor)
        view.layer?.contentsScale = window.backingScaleFactor
        view.layer?.magnificationFilter = .nearest

        guard let spawned = spawnHost(
            cols: cols, rows: rows, fontSize: 16 * scale, command: command)
        else {
            NSApp.terminate(nil)
            return
        }
        host = spawned
        view.onKeyBytes = { [weak self] bytes in
            guard let host = self?.host else { return }
            _ = bytes.withUnsafeBufferPointer { buffer in
                ruuah_host_send(host, buffer.baseAddress, buffer.count)
            }
        }

        // For screenshots: `screencapture -l` takes this id.
        print("RUUAH_HOST_WINDOW=\(window.windowNumber)")

        timer = Timer.scheduledTimer(withTimeInterval: 1.0 / 60.0, repeats: true) { [weak self] _ in
            self?.poll()
        }
    }

    private func poll() {
        guard let host else { return }
        var frame = RuuahHostFrame()
        guard ruuah_host_poll(host, &frame) == RUUAH_HOST_SUCCESS else { return }

        if frame.drew, let pixels = frame.pixels {
            blit(pixels, width: Int(frame.width), height: Int(frame.height))
            if cellWidth == 0 {
                // The C surface reports pixels, not metrics; the cell size is derived once
                // from the first frame and the geometry this side chose.
                cellWidth = Int(frame.width) / Int(cols)
                cellHeight = Int(frame.height) / Int(rows)
                sizeWindowToGrid()
            }
        }
        if frame.child_exited {
            NSApp.terminate(nil)
        }
    }

    private func blit(_ pixels: UnsafePointer<UInt8>, width: Int, height: Int) {
        // Copied because the borrow dies at the next poll, and CoreGraphics reads lazily.
        let data = Data(bytes: pixels, count: width * height * 4)
        guard let provider = CGDataProvider(data: data as CFData),
            let image = CGImage(
                width: width,
                height: height,
                bitsPerComponent: 8,
                bitsPerPixel: 32,
                bytesPerRow: width * 4,
                space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue),
                provider: provider,
                decode: nil,
                shouldInterpolate: false,
                intent: .defaultIntent
            )
        else { return }
        view.layer?.contents = image
    }

    /// Sizes the window so the content view is exactly the grid, in points.
    private func sizeWindowToGrid() {
        let scale = window.backingScaleFactor
        let size = NSSize(
            width: CGFloat(cellWidth * Int(cols)) / scale,
            height: CGFloat(cellHeight * Int(rows)) / scale
        )
        window.setContentSize(size)
        window.contentResizeIncrements = NSSize(
            width: CGFloat(cellWidth) / scale, height: CGFloat(cellHeight) / scale)
    }

    func windowDidEndLiveResize(_ notification: Notification) {
        guard let host, cellWidth > 0 else { return }
        let scale = window.backingScaleFactor
        let backing = NSSize(
            width: view.bounds.width * scale, height: view.bounds.height * scale)
        let newCols = UInt16(max(2, Int(backing.width) / cellWidth))
        let newRows = UInt16(max(2, Int(backing.height) / cellHeight))
        guard newCols != cols || newRows != rows else { return }
        if ruuah_host_resize(host, newCols, newRows) == RUUAH_HOST_SUCCESS {
            cols = newCols
            rows = newRows
        }
    }

    func windowWillClose(_ notification: Notification) {
        NSApp.terminate(nil)
    }

    func applicationWillTerminate(_ notification: Notification) {
        timer?.invalidate()
        if let host {
            ruuah_host_free(host)
            self.host = nil
        }
    }
}
