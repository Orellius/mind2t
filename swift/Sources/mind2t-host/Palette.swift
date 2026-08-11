// The cmd+K command palette (S3): app actions + workflow templates, keyboard-driven.
//
// One overlay, two stages. Stage one filters a flat item list (actions first, then
// workflows); stage two walks a chosen workflow's placeholders ONE FIELD AT A TIME --
// fewer moving parts than a form, and the whole flow stays on the keyboard: type to
// filter, up/down to select, Enter to run or advance, Esc to back out. The rendered
// command goes through the PASTE path, never executed -- the user's own Enter in the
// shell runs it, which is what keeps a template with a typo recoverable.

import AppKit
import CMind2tHost

/// Swift face of the Mind2tWorkflows C handle. Loaded fresh at every palette open so
/// file edits show up without a restart; freed with the palette.
final class Workflows {
    private var handle: OpaquePointer?

    /// `dir` nil = the real ~/.ruuah/workflows; a test or capture passes its own.
    init(dir: String? = nil) {
        var out: OpaquePointer?
        if let dir {
            _ = dir.withCString { pointer in mind2t_workflows_load(pointer, &out) }
        } else {
            _ = mind2t_workflows_load(nil, &out)
        }
        handle = out
    }

    deinit {
        mind2t_workflows_free(handle)
    }

    var count: Int { Int(mind2t_workflows_count(handle)) }

    var errors: String {
        var length = 0
        guard mind2t_workflows_errors(handle, nil, 0, &length) == MIND2T_HOST_SUCCESS,
            length > 0
        else { return "" }
        var buffer = [UInt8](repeating: 0, count: length)
        guard
            buffer.withUnsafeMutableBufferPointer({ pointer in
                mind2t_workflows_errors(handle, pointer.baseAddress, length, &length)
            }) == MIND2T_HOST_SUCCESS
        else { return "" }
        return String(decoding: buffer, as: UTF8.self)
    }

    func field(_ index: Int, _ field: UInt32) -> String {
        var length = 0
        guard
            mind2t_workflow_field(handle, UInt32(index), field, nil, 0, &length)
                == MIND2T_HOST_SUCCESS, length > 0
        else { return "" }
        var buffer = [UInt8](repeating: 0, count: length)
        guard
            buffer.withUnsafeMutableBufferPointer({ pointer in
                mind2t_workflow_field(
                    handle, UInt32(index), field, pointer.baseAddress, length, &length)
            }) == MIND2T_HOST_SUCCESS
        else { return "" }
        return String(decoding: buffer, as: UTF8.self)
    }

    func argCount(_ index: Int) -> Int {
        Int(mind2t_workflow_arg_count(handle, UInt32(index)))
    }

    /// nil = the C surface answered Ignored: no default exists, prompt bare.
    func arg(_ index: Int, _ argIndex: Int, _ field: UInt32) -> String? {
        var length = 0
        let sized = mind2t_workflow_arg(
            handle, UInt32(index), UInt32(argIndex), field, nil, 0, &length)
        guard sized == MIND2T_HOST_SUCCESS else { return nil }
        if length == 0 { return "" }
        var buffer = [UInt8](repeating: 0, count: length)
        guard
            buffer.withUnsafeMutableBufferPointer({ pointer in
                mind2t_workflow_arg(
                    handle, UInt32(index), UInt32(argIndex), field, pointer.baseAddress,
                    length, &length)
            }) == MIND2T_HOST_SUCCESS
        else { return nil }
        return String(decoding: buffer, as: UTF8.self)
    }

    /// The substituted command, or nil when a placeholder is unresolved -- the C
    /// surface refuses those, and the palette keeps the user in the field instead.
    func render(_ index: Int, values: [(String, String)]) -> [UInt8]? {
        var blob: [UInt8] = []
        for (name, value) in values {
            blob.append(contentsOf: Array(name.utf8))
            blob.append(0)
            blob.append(contentsOf: Array(value.utf8))
            blob.append(0)
        }
        var length = 0
        let sized = blob.withUnsafeBufferPointer { pointer in
            mind2t_workflow_render(
                handle, UInt32(index), pointer.baseAddress, pointer.count, nil, 0, &length)
        }
        guard sized == MIND2T_HOST_SUCCESS, length > 0 else { return nil }
        var out = [UInt8](repeating: 0, count: length)
        let copied = blob.withUnsafeBufferPointer { pointer in
            out.withUnsafeMutableBufferPointer { outPointer in
                mind2t_workflow_render(
                    handle, UInt32(index), pointer.baseAddress, pointer.count,
                    outPointer.baseAddress, length, &length)
            }
        }
        guard copied == MIND2T_HOST_SUCCESS else { return nil }
        return out
    }
}

/// One selectable row: an app action, or a workflow (which opens the parameter stage).
struct PaletteItem {
    let title: String
    let subtitle: String
    let workflowIndex: Int?
    let action: (() -> Void)?
}

final class PaletteView: NSView, NSTextFieldDelegate {
    /// Restore focus to the terminal; the palette never owns dismissal policy.
    var onDismiss: (() -> Void)?
    /// The rendered workflow command, for the paste path.
    var onCommand: (([UInt8]) -> Void)?

    private let items: [PaletteItem]
    private let workflows: Workflows
    private let field = NSTextField()
    private let hint = NSTextField(labelWithString: "")
    private let list = NSStackView()
    private var filtered: [PaletteItem] = []
    private var selected = 0

    /// Parameter stage: the chosen workflow and the values collected so far.
    private var paramWorkflow: Int?
    private var paramIndex = 0
    private var paramValues: [(String, String)] = []

    private static let accent = NSColor(
        srgbRed: 0x58 / 255.0, green: 0x65 / 255.0, blue: 0xF2 / 255.0, alpha: 1)

    init(items: [PaletteItem], workflows: Workflows) {
        self.items = items
        self.workflows = workflows
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor =
            NSColor(srgbRed: 0.08, green: 0.08, blue: 0.10, alpha: 0.98).cgColor
        layer?.cornerRadius = 10
        layer?.borderWidth = 1
        layer?.borderColor = NSColor(white: 1, alpha: 0.12).cgColor

        field.isBordered = false
        field.isBezeled = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = NSFont.monospacedSystemFont(ofSize: 15, weight: .regular)
        field.textColor = .white
        field.placeholderString = "Type a command or workflow…"
        field.delegate = self

        hint.font = NSFont.systemFont(ofSize: 11)
        hint.textColor = NSColor(white: 1, alpha: 0.45)
        hint.lineBreakMode = .byTruncatingTail

        list.orientation = .vertical
        list.alignment = .leading
        list.spacing = 2

        for view in [field, hint, list] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        NSLayoutConstraint.activate([
            field.topAnchor.constraint(equalTo: topAnchor, constant: 14),
            field.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 16),
            field.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -16),
            hint.topAnchor.constraint(equalTo: field.bottomAnchor, constant: 6),
            hint.leadingAnchor.constraint(equalTo: field.leadingAnchor),
            hint.trailingAnchor.constraint(equalTo: field.trailingAnchor),
            list.topAnchor.constraint(equalTo: hint.bottomAnchor, constant: 8),
            list.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            list.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            list.bottomAnchor.constraint(lessThanOrEqualTo: bottomAnchor, constant: -10),
        ])
        refilter()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not from a nib") }

    func focus() {
        window?.makeFirstResponder(field)
    }

    // MARK: stage one -- filter and select

    private func refilter() {
        let query = field.stringValue.lowercased()
        filtered = query.isEmpty
            ? items
            : items.filter {
                $0.title.lowercased().contains(query)
                    || $0.subtitle.lowercased().contains(query)
            }
        selected = 0
        hint.stringValue = "\u{2191}\u{2193} select \u{2022} \u{21A9} run \u{2022} esc close"
        rebuildRows()
    }

    private func rebuildRows() {
        for view in list.arrangedSubviews {
            list.removeArrangedSubview(view)
            view.removeFromSuperview()
        }
        for (index, item) in filtered.prefix(10).enumerated() {
            let title = NSTextField(labelWithString: item.title)
            title.font = NSFont.monospacedSystemFont(ofSize: 13, weight: .medium)
            title.textColor = index == selected ? .white : NSColor(white: 1, alpha: 0.85)
            let subtitle = NSTextField(labelWithString: item.subtitle)
            subtitle.font = NSFont.systemFont(ofSize: 11)
            subtitle.textColor = NSColor(white: 1, alpha: 0.45)
            subtitle.lineBreakMode = .byTruncatingTail

            let row = NSStackView(views: [title, subtitle])
            row.orientation = .horizontal
            row.spacing = 8
            row.edgeInsets = NSEdgeInsets(top: 4, left: 8, bottom: 4, right: 8)
            row.wantsLayer = true
            row.layer?.cornerRadius = 6
            if index == selected {
                row.layer?.backgroundColor = PaletteView.accent.withAlphaComponent(0.35).cgColor
            }
            row.translatesAutoresizingMaskIntoConstraints = false
            list.addArrangedSubview(row)
            row.widthAnchor.constraint(equalTo: list.widthAnchor).isActive = true
        }
        invalidateIntrinsicContentSize()
    }

    override var intrinsicContentSize: NSSize {
        NSSize(width: 560, height: 66 + CGFloat(min(filtered.count, 10)) * 28)
    }

    // MARK: stage two -- one placeholder at a time

    private func beginParams(workflow: Int) {
        paramWorkflow = workflow
        paramIndex = 0
        paramValues = []
        promptForCurrentParam()
    }

    private func promptForCurrentParam() {
        guard let workflow = paramWorkflow else { return }
        let name = workflows.arg(workflow, paramIndex, MIND2T_WORKFLOW_NAME) ?? "?"
        let description =
            workflows.arg(workflow, paramIndex, MIND2T_WORKFLOW_DESCRIPTION) ?? ""
        let total = workflows.argCount(workflow)
        field.stringValue = workflows.arg(workflow, paramIndex, MIND2T_WORKFLOW_ARG_DEFAULT) ?? ""
        field.placeholderString = name
        hint.stringValue = [
            "\(workflows.field(workflow, MIND2T_WORKFLOW_NAME)) \u{2022} \(paramIndex + 1)/\(total): \(name)",
            description,
        ]
        .filter { !$0.isEmpty }
        .joined(separator: " \u{2014} ")
        filtered = []
        rebuildRows()
        field.currentEditor()?.selectAll(nil)
    }

    private func acceptCurrentParam() {
        guard let workflow = paramWorkflow else { return }
        let name = workflows.arg(workflow, paramIndex, MIND2T_WORKFLOW_NAME) ?? ""
        paramValues.append((name, field.stringValue))
        paramIndex += 1
        if paramIndex < workflows.argCount(workflow) {
            promptForCurrentParam()
            return
        }
        if let bytes = workflows.render(workflow, values: paramValues) {
            onCommand?(bytes)
            onDismiss?()
        } else {
            // A refused render (unresolved placeholder) keeps the palette up; the
            // hint says why instead of typing a broken command into the shell.
            hint.stringValue = "render refused: a placeholder has no value"
        }
    }

    private func runSelected() {
        guard selected < filtered.count else { return }
        let item = filtered[selected]
        if let workflow = item.workflowIndex {
            field.stringValue = ""
            beginParams(workflow: workflow)
        } else {
            onDismiss?()
            item.action?()
        }
    }

    // MARK: keyboard

    func controlTextDidChange(_ notification: Notification) {
        if paramWorkflow == nil {
            refilter()
        }
    }

    func control(
        _ control: NSControl, textView: NSTextView, doCommandBy selector: Selector
    ) -> Bool {
        switch selector {
        case #selector(NSResponder.moveUp(_:)):
            if paramWorkflow == nil, !filtered.isEmpty {
                selected = (selected + filtered.prefix(10).count - 1) % filtered.prefix(10).count
                rebuildRows()
            }
            return true
        case #selector(NSResponder.moveDown(_:)):
            if paramWorkflow == nil, !filtered.isEmpty {
                selected = (selected + 1) % filtered.prefix(10).count
                rebuildRows()
            }
            return true
        case #selector(NSResponder.insertNewline(_:)):
            if paramWorkflow == nil {
                runSelected()
            } else {
                acceptCurrentParam()
            }
            return true
        case #selector(NSResponder.cancelOperation(_:)):
            if paramWorkflow != nil {
                // Back to stage one with the list restored, not straight out.
                paramWorkflow = nil
                field.stringValue = ""
                field.placeholderString = "Type a command or workflow…"
                refilter()
            } else {
                onDismiss?()
            }
            return true
        default:
            return false
        }
    }
}
