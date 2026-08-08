// The top tab strip, 1:1 to the operator's reference (Warp's bar, 2026-07-30):
// traffic lights inline at the left, pill tabs with an icon and a label, the ACTIVE
// tab on a dark green pill with green text and an X at the label's left, a "+" after
// the last tab, and a panel-toggle glyph at the far right. Collaborator avatars from
// the reference are omitted -- sessions have no collaborators -- flagged as the one
// deviation. Colors sampled from the reference; aesthetic verdict is the operator's.

import AppKit

protocol TabBarDelegate: AnyObject {
    func tabBarDidSelect(index: Int)
    func tabBarDidRequestNew()
    func tabBarDidRequestClose(index: Int)
    /// Whether the sidebar toggle should do anything (S5.5: only with panels enabled).
    func tabBarCanToggleSidebar() -> Bool
    func tabBarDidToggleSidebar()
}

final class TabBarView: NSView {
    static let height: CGFloat = 38

    weak var delegate: TabBarDelegate?

    private var titles: [String] = []
    private var states: [Session.WorkState] = []
    private var activeIndex = -1
    private var pills: [NSView] = []

    // Reference palette: near-black bar, graphite pills, emerald active.
    private static let barColor = NSColor(srgbRed: 0x0B / 255, green: 0x0D / 255, blue: 0x12 / 255, alpha: 1)
    private static let pillColor = NSColor(srgbRed: 0x1E / 255, green: 0x22 / 255, blue: 0x2B / 255, alpha: 1)
    private static let activePill = NSColor(srgbRed: 0x0F / 255, green: 0x2A / 255, blue: 0x1F / 255, alpha: 1)
    private static let labelColor = NSColor(srgbRed: 0xC9 / 255, green: 0xCE / 255, blue: 0xD8 / 255, alpha: 1)
    private static let activeGreen = NSColor(srgbRed: 0x2B / 255, green: 0xD9 / 255, blue: 0x9F / 255, alpha: 1)

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = TabBarView.barColor.cgColor
    }

    required init?(coder: NSCoder) { nil }

    func update(titles: [String], states: [Session.WorkState], activeIndex: Int) {
        guard titles != self.titles || states != self.states || activeIndex != self.activeIndex
        else { return }
        self.titles = titles
        self.states = states
        self.activeIndex = activeIndex
        rebuild()
    }

    override func layout() {
        super.layout()
        rebuild()
    }

    private func rebuild() {
        for pill in pills { pill.removeFromSuperview() }
        pills.removeAll()

        // Traffic lights occupy roughly the first 78 points of a titled window whose
        // title bar is transparent; the first pill starts past them.
        var x: CGFloat = 86
        let pillHeight: CGFloat = 26
        let y = (bounds.height - pillHeight) / 2

        for (index, title) in titles.enumerated() {
            let active = index == activeIndex
            let pill = makePill(
                title: title, active: active, index: index, height: pillHeight,
                state: index < states.count ? states[index] : .idle)
            pill.frame.origin = NSPoint(x: x, y: y)
            addSubview(pill)
            pills.append(pill)
            x += pill.frame.width + 8
        }

        // "+" after the last tab.
        let plus = makeButton(symbol: "plus", action: #selector(plusClicked))
        plus.frame = NSRect(x: x + 2, y: y, width: 26, height: pillHeight)
        addSubview(plus)
        pills.append(plus)

        // The workspace sidebar toggle, pinned right as in the reference. It carried the
        // reference's shape and no behaviour until S5.5; `nil` action when panels are off
        // keeps it inert rather than opening something that cannot exist.
        let panel = makeButton(
            symbol: "sidebar.right",
            action: delegate?.tabBarCanToggleSidebar() == true ? #selector(toggleSidebar) : nil)
        panel.frame = NSRect(x: bounds.width - 38, y: y, width: 26, height: pillHeight)
        panel.autoresizingMask = [.minXMargin]
        addSubview(panel)
        pills.append(panel)
    }

    // Work-state indicator colors: amber while an agent works, red on error, green
    // when finished and not yet looked at. Explicit signals only -- OSC 9;4 progress
    // or a per-CLI title marker -- never an idle timer (the operator's rule).
    private static let workingDot = NSColor(srgbRed: 1.0, green: 0.72, blue: 0.20, alpha: 1)
    private static let errorDot = NSColor(srgbRed: 0.95, green: 0.30, blue: 0.30, alpha: 1)
    private static let doneDot = NSColor(srgbRed: 0x2B / 255, green: 0xD9 / 255, blue: 0x9F / 255, alpha: 1)

    private func makePill(
        title: String, active: Bool, index: Int, height: CGFloat, state: Session.WorkState
    ) -> NSView {
        let shown = title.count > 24 ? String(title.prefix(23)) + "…" : title
        let label = NSTextField(labelWithString: shown)
        label.font = NSFont.systemFont(ofSize: 12, weight: .medium)
        label.textColor = active ? TabBarView.activeGreen : TabBarView.labelColor
        label.sizeToFit()

        let hasClose = active
        let iconWidth: CGFloat = 16
        let closeWidth: CGFloat = hasClose ? 16 : 0
        let dotWidth: CGFloat = state == .idle ? 0 : 12
        let padding: CGFloat = 10
        let width =
            padding + closeWidth + iconWidth + 6 + label.frame.width + dotWidth + padding

        let pill = ClickView(frame: NSRect(x: 0, y: 0, width: width, height: height))
        pill.wantsLayer = true
        pill.layer?.backgroundColor =
            (active ? TabBarView.activePill : TabBarView.pillColor).cgColor
        pill.layer?.cornerRadius = 7
        pill.onClick = { [weak self] in self?.delegate?.tabBarDidSelect(index: index) }

        var cursor = padding
        if hasClose {
            // The X sits at the LABEL'S LEFT on the active tab, exactly as the
            // reference draws it.
            let close = NSButton(
                image: NSImage(systemSymbolName: "xmark", accessibilityDescription: "close")!,
                target: self, action: #selector(closeClicked(_:)))
            close.tag = index
            close.isBordered = false
            close.contentTintColor = TabBarView.activeGreen
            close.frame = NSRect(x: cursor - 4, y: (height - 14) / 2, width: 16, height: 14)
            pill.addSubview(close)
            cursor += closeWidth
        }
        let icon = NSImageView(
            image: NSImage(systemSymbolName: "terminal", accessibilityDescription: nil)!)
        icon.contentTintColor = active ? TabBarView.activeGreen : TabBarView.labelColor
        icon.frame = NSRect(x: cursor, y: (height - 14) / 2, width: iconWidth, height: 14)
        pill.addSubview(icon)
        cursor += iconWidth + 6
        label.frame.origin = NSPoint(x: cursor, y: (height - label.frame.height) / 2)
        pill.addSubview(label)
        cursor += label.frame.width
        if state != .idle {
            let dot = NSView(frame: NSRect(x: cursor + 5, y: (height - 7) / 2, width: 7, height: 7))
            dot.wantsLayer = true
            dot.layer?.cornerRadius = 3.5
            dot.layer?.backgroundColor = {
                switch state {
                case .working: return TabBarView.workingDot.cgColor
                case .error: return TabBarView.errorDot.cgColor
                case .done: return TabBarView.doneDot.cgColor
                case .idle: return NSColor.clear.cgColor
                }
            }()
            pill.addSubview(dot)
        }
        return pill
    }

    private func makeButton(symbol: String, action: Selector?) -> NSButton {
        let button = NSButton(
            image: NSImage(systemSymbolName: symbol, accessibilityDescription: symbol)!,
            target: action == nil ? nil : self, action: action)
        button.isBordered = false
        button.contentTintColor = TabBarView.labelColor
        return button
    }

    @objc private func plusClicked() {
        delegate?.tabBarDidRequestNew()
    }

    @objc private func closeClicked(_ sender: NSButton) {
        delegate?.tabBarDidRequestClose(index: sender.tag)
    }

    @objc private func toggleSidebar() {
        delegate?.tabBarDidToggleSidebar()
    }
}

/// A view that reports a plain click; tabs must not steal first responder from the grid.
private final class ClickView: NSView {
    var onClick: (() -> Void)?
    override var acceptsFirstResponder: Bool { false }
    override func mouseDown(with event: NSEvent) {
        onClick?()
    }
}
