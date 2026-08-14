// The docked workspace sidebar: one row per session, with the metadata the tab strip
// has no room for.
//
// This is the SECOND sidebar. S5.5 built one, its contents were replaced by the tab
// strip on 2026-07-30, and Orel deleted the empty shell on 2026-08-11 (5805132) with
// the condition for its return stated in that commit: "either you remove that right
// sidebar or get it used with something". This one is built to that condition, so the
// rule it must keep obeying is that every row shows something the strip cannot.
//
// What the strip cannot show, and why each fact is here rather than there: a pill is
// sized to its label, so a strip of six sessions has room for six words and nothing
// else. A row has two lines. The second line carries the branch and the directory, and
// those are the two facts that tell one agent session from another when both are
// running `claude` in the same repository.
//
// Deliberately NOT here: a toggle button, a slide animation and its re-entrancy guard.
// Those were the maintenance cost named in the deletion commit, and none of them is
// required to show rows. The sidebar is docked or absent, decided once at launch.

import AppKit

/// One session, as the sidebar sees it. A value type so `update` can diff cheaply and
/// skip the rebuild entirely - the refresh is driven by title and progress events, and
/// a busy agent emits those several times a second.
struct SidebarRow: Equatable {
    /// Workspace name when the session has one, else the session's live title.
    let title: String
    /// `branch · directory`, either half optional. Empty when we know neither.
    let subtitle: String
    let state: Session.WorkState
}

protocol SidebarDelegate: AnyObject {
    func sidebarDidSelect(index: Int)
    /// The `+` was hit. `origin` is in screen coordinates, for popping a menu under it.
    func sidebarDidRequestNew(from view: NSView)
}

final class SidebarView: NSView {
    /// Preferred width. `ChromeLayout` treats this as a REQUEST, not a guarantee: the
    /// pane's minimum wins on a narrow window and the sidebar takes the remainder.
    static let preferredWidth: CGFloat = 260

    static let rowHeight: CGFloat = 46
    private static let headerHeight: CGFloat = 26

    weak var delegate: SidebarDelegate?

    private var rows: [SidebarRow] = []
    private var activeIndex = -1
    private var built: [NSView] = []

    // Sampled from the same reference as the tab strip so the two read as one surface.
    //
    // DUPLICATED from TabBarView's private palette, deliberately and with the cost
    // stated: unifying them means a third file, and these two views are the ones most
    // likely to diverge visually on Orel's word. A third consumer is the signal to
    // extract a palette type rather than copy a third time.
    private static let barColor = NSColor(srgbRed: 0x0B / 255, green: 0x0D / 255, blue: 0x12 / 255, alpha: 1)
    private static let rowColor = NSColor(srgbRed: 0x1E / 255, green: 0x22 / 255, blue: 0x2B / 255, alpha: 1)
    private static let activeRow = NSColor(srgbRed: 0x0F / 255, green: 0x2A / 255, blue: 0x1F / 255, alpha: 1)
    private static let labelColor = NSColor(srgbRed: 0xC9 / 255, green: 0xCE / 255, blue: 0xD8 / 255, alpha: 1)
    private static let mutedColor = NSColor(srgbRed: 0x6B / 255, green: 0x72 / 255, blue: 0x80 / 255, alpha: 1)
    private static let activeGreen = NSColor(srgbRed: 0x2B / 255, green: 0xD9 / 255, blue: 0x9F / 255, alpha: 1)
    private static let workingDot = NSColor(srgbRed: 1.0, green: 0.72, blue: 0.20, alpha: 1)
    private static let errorDot = NSColor(srgbRed: 0.95, green: 0.30, blue: 0.30, alpha: 1)

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = SidebarView.barColor.cgColor
    }

    required init?(coder: NSCoder) { nil }

    /// The grid must keep first responder. A sidebar that takes it swallows the next
    /// keystroke into nothing, which reads as the terminal having frozen.
    override var acceptsFirstResponder: Bool { false }

    func update(rows: [SidebarRow], activeIndex: Int) {
        guard rows != self.rows || activeIndex != self.activeIndex else { return }
        self.rows = rows
        self.activeIndex = activeIndex
        rebuild()
    }

    override func layout() {
        super.layout()
        rebuild()
    }

    /// How many rows fit at the current height. Exposed so a gate can assert the
    /// truncation limit is what this file says it is, rather than trusting the comment.
    var visibleRowCapacity: Int {
        let usable = bounds.height - SidebarView.headerHeight
        guard usable > 0 else { return 0 }
        return max(0, Int(usable / SidebarView.rowHeight))
    }

    private func rebuild() {
        for view in built { view.removeFromSuperview() }
        built.removeAll()

        // Top-down, so adding a session grows the list downward. AppKit's origin is
        // bottom-left, hence the subtraction rather than an accumulator.
        var top = bounds.height - SidebarView.headerHeight
        let header = makeHeader(y: top)
        addSubview(header)
        built.append(header)

        for (index, row) in rows.enumerated() {
            top -= SidebarView.rowHeight
            // Rows past the bottom edge are simply not built. This is a stated LIMIT,
            // not a scroll implementation: past `visibleRowCapacity` sessions the tail
            // is invisible. A scroll view is the fix and it is deferred, not forgotten.
            if top < 0 { break }
            let view = makeRow(row, index: index, active: index == activeIndex)
            view.frame = NSRect(
                x: 8, y: top, width: max(0, bounds.width - 16), height: SidebarView.rowHeight - 6)
            addSubview(view)
            built.append(view)
        }
    }

    private func makeHeader(y: CGFloat) -> NSView {
        let header = NSView(frame: NSRect(
            x: 0, y: y, width: bounds.width, height: SidebarView.headerHeight))

        let label = NSTextField(labelWithString: "WORKSPACES")
        label.font = NSFont.systemFont(ofSize: 10, weight: .semibold)
        label.textColor = SidebarView.mutedColor
        label.sizeToFit()
        label.frame.origin = NSPoint(x: 14, y: 6)
        header.addSubview(label)

        // The `+`. Before this, every way to open a session lived behind cmd+K, so the
        // feature was invisible to anyone who had not already been told it existed.
        // Orel's call, 2026-08-14, and the discoverability argument is the whole reason.
        let plus = SidebarPlusButton(
            frame: NSRect(
                x: max(0, bounds.width - SidebarView.plusSize - 8), y: 3,
                width: SidebarView.plusSize, height: SidebarView.plusSize))
        plus.onClick = { [weak self] view in
            guard let self else { return }
            self.delegate?.sidebarDidRequestNew(from: view)
        }
        header.addSubview(plus)
        return header
    }

    static let plusSize: CGFloat = 20

    private func makeRow(_ row: SidebarRow, index: Int, active: Bool) -> NSView {
        let container = SidebarRowView(frame: .zero)
        container.wantsLayer = true
        container.layer?.backgroundColor =
            (active ? SidebarView.activeRow : SidebarView.rowColor).cgColor
        container.layer?.cornerRadius = 7
        container.onClick = { [weak self] in self?.delegate?.sidebarDidSelect(index: index) }

        let title = NSTextField(labelWithString: row.title)
        title.font = NSFont.systemFont(ofSize: 12, weight: .medium)
        title.textColor = active ? SidebarView.activeGreen : SidebarView.labelColor
        title.lineBreakMode = .byTruncatingTail
        container.addSubview(title)

        let subtitle = NSTextField(labelWithString: row.subtitle)
        subtitle.font = NSFont.systemFont(ofSize: 10, weight: .regular)
        subtitle.textColor = SidebarView.mutedColor
        // Truncate at the HEAD: a path's tail identifies it and its leading components
        // are what every row shares. Tail truncation would show `/Users/orel/Pro…` on
        // every row, which is the half that carries no information.
        subtitle.lineBreakMode = .byTruncatingHead
        container.addSubview(subtitle)

        container.titleField = title
        container.subtitleField = subtitle
        if row.state != .idle {
            let dot = makeDot(row.state)
            container.dot = dot
            container.addSubview(dot)
        }
        return container
    }

    private func makeDot(_ state: Session.WorkState) -> NSView {
        let dot = NSView(frame: NSRect(x: 0, y: 0, width: 7, height: 7))
        dot.wantsLayer = true
        dot.layer?.cornerRadius = 3.5
        switch state {
        case .working: dot.layer?.backgroundColor = SidebarView.workingDot.cgColor
        case .error: dot.layer?.backgroundColor = SidebarView.errorDot.cgColor
        case .done: dot.layer?.backgroundColor = SidebarView.activeGreen.cgColor
        case .idle: dot.layer?.backgroundColor = NSColor.clear.cgColor
        }
        return dot
    }
}

/// A row that lays its own children out and reports a plain click.
///
/// It does its own layout rather than using autoresizing masks because the dot's x
/// depends on the row's width, and an autoresized dot drifts off the right edge during
/// a live window drag before any layout pass corrects it.
private final class SidebarRowView: NSView {
    var onClick: (() -> Void)?
    var titleField: NSTextField?
    var subtitleField: NSTextField?
    var dot: NSView?

    override var acceptsFirstResponder: Bool { false }

    override func mouseDown(with event: NSEvent) {
        onClick?()
    }

    override func layout() {
        super.layout()
        let padding: CGFloat = 10
        let dotWidth: CGFloat = dot == nil ? 0 : 14
        let textWidth = max(0, bounds.width - padding * 2 - dotWidth)
        titleField?.frame = NSRect(
            x: padding, y: bounds.height - 21, width: textWidth, height: 16)
        subtitleField?.frame = NSRect(x: padding, y: 5, width: textWidth, height: 14)
        dot?.frame = NSRect(
            x: bounds.width - padding - 7, y: bounds.height - 17, width: 7, height: 7)
    }
}

/// The header's `+`. A drawn glyph rather than an `NSButton` so it wears the sidebar's own
/// palette without fighting AppKit's bezel styles, and so the hit target is the whole
/// square rather than the two strokes.
///
/// Refuses first responder like everything else in this view: the grid holds it, and a
/// chrome control that takes it swallows the next keystroke into nothing, which reads as
/// the terminal having frozen rather than as a focus bug.
private final class SidebarPlusButton: NSView {
    var onClick: ((NSView) -> Void)?
    private var hovering = false
    private var tracking: NSTrackingArea?

    override var acceptsFirstResponder: Bool { false }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 4
        toolTip = "New session, workspace, or SSH connection"
    }

    required init?(coder: NSCoder) { nil }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(
            rect: bounds, options: [.mouseEnteredAndExited, .activeInKeyWindow], owner: self)
        addTrackingArea(area)
        tracking = area
    }

    override func mouseEntered(with event: NSEvent) {
        hovering = true
        needsDisplay = true
    }

    override func mouseExited(with event: NSEvent) {
        hovering = false
        needsDisplay = true
    }

    override func mouseDown(with event: NSEvent) {
        onClick?(self)
    }

    override func draw(_ dirtyRect: NSRect) {
        if hovering {
            NSColor(srgbRed: 0x1E / 255, green: 0x22 / 255, blue: 0x2B / 255, alpha: 1).setFill()
            bounds.fill()
        }
        let color =
            hovering
            ? NSColor(srgbRed: 0x2B / 255, green: 0xD9 / 255, blue: 0x9F / 255, alpha: 1)
            : NSColor(srgbRed: 0x6B / 255, green: 0x72 / 255, blue: 0x80 / 255, alpha: 1)
        color.setStroke()
        let arm: CGFloat = 5
        let mid = NSPoint(x: bounds.midX, y: bounds.midY)
        let path = NSBezierPath()
        path.lineWidth = 1.5
        path.lineCapStyle = .round
        path.move(to: NSPoint(x: mid.x - arm, y: mid.y))
        path.line(to: NSPoint(x: mid.x + arm, y: mid.y))
        path.move(to: NSPoint(x: mid.x, y: mid.y - arm))
        path.line(to: NSPoint(x: mid.x, y: mid.y + arm))
        path.stroke()
    }
}
