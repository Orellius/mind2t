// The session sidebar: a dark source list in the app's own identity, not vibrancy.
//
// A translucent material over a pure-black terminal reads as mud, so the sidebar is a
// solid near-black with the RUUAH pink reserved for the one accent that means "this is
// where new things start". Rows select with a soft rounded fill, the table refuses
// first responder so a click never steals the keyboard from the terminal.

import AppKit

protocol SessionListDelegate: AnyObject {
    func sessionListDidSelect(index: Int)
    func sessionListDidRequestNew()
    func sessionListDidRequestClose(index: Int)
}

final class SessionListController: NSViewController, NSTableViewDataSource, NSTableViewDelegate {
    static let width: CGFloat = 220
    static let background = NSColor(srgbRed: 0.051, green: 0.051, blue: 0.071, alpha: 1)
    static let accent = NSColor(srgbRed: 0.925, green: 0.282, blue: 0.6, alpha: 1)  // #EC4899

    weak var delegate: SessionListDelegate?

    private let tableView = NSTableView()
    private var titles: [String] = []
    private var activeIndex = -1

    override func loadView() {
        let container = NSView()
        container.wantsLayer = true
        container.layer?.backgroundColor = SessionListController.background.cgColor

        // The ghost leads, like the reference: the app icon IS the brand asset, so the
        // bundle needs no second copy and the bare CLI binary degrades to its generic
        // icon instead of a missing image.
        let logo = NSImageView()
        logo.image = NSApp.applicationIconImage
        logo.imageScaling = .scaleProportionallyUpOrDown

        let wordmark = NSTextField(labelWithString: "RUUAH VT")
        wordmark.font = NSFont.systemFont(ofSize: 13, weight: .semibold)
        wordmark.textColor = NSColor.white.withAlphaComponent(0.9)

        let newButton = NSButton(
            title: "  New Session", image: plusImage(), target: self,
            action: #selector(requestNew))
        newButton.imagePosition = .imageLeading
        newButton.isBordered = false
        newButton.contentTintColor = SessionListController.accent
        newButton.font = NSFont.systemFont(ofSize: 13, weight: .medium)
        newButton.alignment = .left

        let column = NSTableColumn(identifier: .init("session"))
        tableView.addTableColumn(column)
        tableView.headerView = nil
        tableView.backgroundColor = .clear
        tableView.rowHeight = 30
        tableView.style = .plain
        tableView.intercellSpacing = NSSize(width: 0, height: 2)
        tableView.selectionHighlightStyle = .regular
        tableView.refusesFirstResponder = true
        tableView.dataSource = self
        tableView.delegate = self
        tableView.target = self
        tableView.action = #selector(rowClicked)
        tableView.menu = closeMenu()

        let scroll = NSScrollView()
        scroll.documentView = tableView
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false

        for subview in [logo, wordmark, newButton, scroll] {
            subview.translatesAutoresizingMaskIntoConstraints = false
            container.addSubview(subview)
        }
        NSLayoutConstraint.activate([
            logo.topAnchor.constraint(equalTo: container.topAnchor, constant: 14),
            logo.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 14),
            logo.widthAnchor.constraint(equalToConstant: 28),
            logo.heightAnchor.constraint(equalToConstant: 28),
            wordmark.centerYAnchor.constraint(equalTo: logo.centerYAnchor),
            wordmark.leadingAnchor.constraint(equalTo: logo.trailingAnchor, constant: 8),
            newButton.topAnchor.constraint(equalTo: logo.bottomAnchor, constant: 16),
            newButton.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 14),
            newButton.trailingAnchor.constraint(
                lessThanOrEqualTo: container.trailingAnchor, constant: -14),
            scroll.topAnchor.constraint(equalTo: newButton.bottomAnchor, constant: 12),
            scroll.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 8),
            scroll.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -8),
            scroll.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -12),
        ])
        view = container
    }

    /// The whole sidebar state arrives at once; the table never owns any of it.
    func update(titles: [String], activeIndex: Int) {
        self.titles = titles
        self.activeIndex = activeIndex
        tableView.reloadData()
        if activeIndex >= 0 && activeIndex < titles.count {
            tableView.selectRowIndexes([activeIndex], byExtendingSelection: false)
        }
    }

    func numberOfRows(in tableView: NSTableView) -> Int { titles.count }

    func tableView(
        _ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int
    ) -> NSView? {
        let cell = NSTableCellView()
        let icon = NSImageView(
            image: NSImage(systemSymbolName: "terminal", accessibilityDescription: nil)
                ?? NSImage())
        icon.contentTintColor =
            row == activeIndex
            ? SessionListController.accent : NSColor.white.withAlphaComponent(0.45)
        let label = NSTextField(labelWithString: titles[row])
        label.font = NSFont.systemFont(ofSize: 13)
        label.textColor =
            row == activeIndex ? .white : NSColor.white.withAlphaComponent(0.65)
        label.lineBreakMode = .byTruncatingTail

        // Every row carries its own close -- the sidebar is the window management here
        // (sessions replace split panes), so closing must not require a keyboard chord
        // or a context menu anyone has to discover.
        let close = NSButton(
            image: NSImage(systemSymbolName: "xmark", accessibilityDescription: "Close session")
                ?? NSImage(),
            target: self, action: #selector(closeRow(_:)))
        close.isBordered = false
        close.tag = row
        close.contentTintColor = NSColor.white.withAlphaComponent(0.35)

        for subview in [icon, label, close] {
            subview.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(subview)
        }
        NSLayoutConstraint.activate([
            icon.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 8),
            icon.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            icon.widthAnchor.constraint(equalToConstant: 16),
            label.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 8),
            label.trailingAnchor.constraint(equalTo: close.leadingAnchor, constant: -6),
            label.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            close.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -8),
            close.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            close.widthAnchor.constraint(equalToConstant: 16),
        ])
        return cell
    }

    @objc private func closeRow(_ sender: NSButton) {
        delegate?.sessionListDidRequestClose(index: sender.tag)
    }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        SidebarRowView()
    }

    @objc private func rowClicked() {
        let row = tableView.clickedRow >= 0 ? tableView.clickedRow : tableView.selectedRow
        guard row >= 0 else { return }
        delegate?.sessionListDidSelect(index: row)
    }

    @objc private func requestNew() {
        delegate?.sessionListDidRequestNew()
    }

    @objc private func closeClickedRow() {
        guard tableView.clickedRow >= 0 else { return }
        delegate?.sessionListDidRequestClose(index: tableView.clickedRow)
    }

    private func closeMenu() -> NSMenu {
        let menu = NSMenu()
        menu.addItem(
            NSMenuItem(
                title: "Close Session", action: #selector(closeClickedRow), keyEquivalent: ""))
        menu.items.forEach { $0.target = self }
        return menu
    }

    private func plusImage() -> NSImage {
        NSImage(systemSymbolName: "plus", accessibilityDescription: "New session") ?? NSImage()
    }
}

/// Selection as a soft rounded fill rather than the system highlight, which would paint
/// the accent color across the whole row and fight the terminal for attention.
private final class SidebarRowView: NSTableRowView {
    override func drawSelection(in dirtyRect: NSRect) {
        guard selectionHighlightStyle != .none else { return }
        let rect = bounds.insetBy(dx: 4, dy: 1)
        let path = NSBezierPath(roundedRect: rect, xRadius: 6, yRadius: 6)
        NSColor.white.withAlphaComponent(0.08).setFill()
        path.fill()
    }
}
