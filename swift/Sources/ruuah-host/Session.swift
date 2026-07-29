// One terminal session: a host handle, its geometry, and the newest image it drew.
//
// The app holds an array of these and blits whichever is active. A background session
// keeps running -- its pty child and pump thread never pause -- and polling it stays
// cheap because an unchanged frame comes back without drawing.

import AppKit
import CRuuahHost

final class Session {
    let title: String
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
    /// One OSC 133 class byte per grid row (RUUAH_ROW_*), from the last drawn frame.
    /// Empty until a shell with integration has marked something. The gutter's input.
    private(set) var rowClasses: [UInt8] = []
    private(set) var exited = false

    init?(
        command: String?, cols: UInt16, rows: UInt16, fontSize: Float, autoDirection: Bool,
        config: OpaquePointer? = nil, title: String
    ) {
        guard
            let host = spawnHost(
                cols: cols, rows: rows, fontSize: fontSize, command: command,
                autoDirection: autoDirection, config: config)
        else { return nil }
        self.host = host
        self.cols = cols
        self.rows = rows
        self.title = title
    }

    /// Polls once. Returns a fresh image when the host drew; `exited` flips when the
    /// child is gone. Safe to call at any rate -- unchanged frames cost almost nothing.
    @discardableResult
    func poll() -> CGImage? {
        guard let host, !exited else { return nil }
        var frame = RuuahHostFrame()
        guard ruuah_host_poll(host, &frame) == RUUAH_HOST_SUCCESS else { return nil }

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
            if cellWidth == 0 {
                cellWidth = Int(frame.width) / Int(cols)
                cellHeight = Int(frame.height) / Int(rows)
            }
        }
        if let semantics = frame.row_semantics, frame.row_count > 0 {
            rowClasses = Array(UnsafeBufferPointer(start: semantics, count: Int(frame.row_count)))
        }
        if frame.child_exited { exited = true }
        return fresh
    }

    /// One row's text, filtered by OSC 133 mark (RUUAH_TEXT_ALL for every cell). Empty
    /// when the row is out of range or nothing has been polled yet.
    func rowText(_ row: UInt16, semantic: UInt8) -> String {
        guard let host else { return "" }
        var length = 0
        guard ruuah_host_row_text(host, row, semantic, nil, 0, &length) == RUUAH_HOST_SUCCESS,
            length > 0
        else { return "" }
        var buffer = [UInt8](repeating: 0, count: length)
        guard
            buffer.withUnsafeMutableBufferPointer({ pointer in
                ruuah_host_row_text(host, row, semantic, pointer.baseAddress, length, &length)
            }) == RUUAH_HOST_SUCCESS
        else { return "" }
        return String(decoding: buffer, as: UTF8.self)
    }

    func send(_ bytes: [UInt8]) {
        guard let host else { return }
        _ = bytes.withUnsafeBufferPointer { buffer in
            ruuah_host_send(host, buffer.baseAddress, buffer.count)
        }
    }

    func paste(_ bytes: [UInt8]) {
        guard let host else { return }
        _ = bytes.withUnsafeBufferPointer { buffer in
            ruuah_host_paste(host, buffer.baseAddress, buffer.count)
        }
    }

    func resize(cols: UInt16, rows: UInt16) {
        guard let host, cols != self.cols || rows != self.rows else { return }
        if ruuah_host_resize(host, cols, rows) == RUUAH_HOST_SUCCESS {
            self.cols = cols
            self.rows = rows
        }
    }

    func close() {
        if let host {
            ruuah_host_free(host)
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
