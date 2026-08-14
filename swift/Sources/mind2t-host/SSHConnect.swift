// The full SSH connection form.
//
// Orel asked for this shape directly on 2026-08-14, over a narrower one-field proposal:
// "a + button in the sidebar plus modal for SSH full connection like every convenient
// software". It is his call and this is built to it.
//
// The design constraint that survived the debate is where the result GOES. Everything
// collected here is either handed straight to a child process or appended to
// `~/.ssh/config`. There is no app-side host store, so there is still exactly one
// inventory of machines and it is the one ssh already reads. A saved host appears in the
// sidebar on the next open because the parser reads the same file back.
//
// THERE IS NO PASSWORD FIELD. `ssh` accepts no password on its command line, so the field
// would have to be stored or piped into a pty, and both are worse than letting ssh prompt
// in the pane where the operator can see what is asking.
//
// The form is GUI seam and closes as `[untested - needs your eyes]` by the repo's own
// rule. What IS gated is the mapping from fields to `SSHConnection`, because a swapped
// pair there sends the port into the user field and nothing visibly breaks.

import AppKit

/// What the operator chose to do with the form.
enum SSHConnectOutcome {
    case cancelled
    /// Dial it now, save nothing.
    case connect(SSHConnection)
    /// Append a `Host` block, then dial the alias so ssh resolves it from the file.
    case saveAndConnect(SSHConnection)
}

/// The grid of fields. Separate from the alert that hosts it so a gate can build one,
/// fill it, and read back the `SSHConnection` without a window ever appearing.
final class SSHConnectForm: NSView {
    private let name = NSTextField()
    private let host = NSTextField()
    private let user = NSTextField()
    private let port = NSTextField()
    private let identity = NSTextField()
    private let jump = NSTextField()
    private let command = NSTextField()
    private let options = NSTextView()

    /// The field the alert should focus first. The host is the only required one.
    var firstField: NSView { host }

    init() {
        super.init(frame: NSRect(x: 0, y: 0, width: 460, height: 300))

        for (field, hint) in [
            (name, "optional, defaults to the host"),
            (host, "required, e.g. 10.0.0.9 or box.example.net"),
            (user, "defaults to your local username"),
            (port, "22"),
            (identity, "~/.ssh/id_ed25519"),
            (jump, "user@bastion"),
            (command, "runs instead of a login shell"),
        ] {
            field.placeholderString = hint
            field.font = NSFont.systemFont(ofSize: 12)
        }

        // A browse button because the interesting directory is `~/.ssh`, which is hidden,
        // so typing the path is the only alternative and it is the one people get wrong.
        let browse = NSButton(title: "Choose...", target: self, action: #selector(chooseIdentity))
        browse.bezelStyle = .rounded
        // The button keeps its natural width and the path field takes the remainder. Left
        // equal, AppKit splits the row and the path -- the long value -- gets half of it.
        browse.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        browse.setContentCompressionResistancePriority(.required, for: .horizontal)
        let identityRow = NSStackView(views: [identity, browse])
        identityRow.orientation = .horizontal
        identityRow.distribution = .fill
        identityRow.spacing = 6

        options.font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        options.isRichText = false
        options.isAutomaticQuoteSubstitutionEnabled = false
        let optionsScroll = NSScrollView()
        optionsScroll.documentView = options
        optionsScroll.hasVerticalScroller = true
        optionsScroll.borderType = .bezelBorder
        optionsScroll.translatesAutoresizingMaskIntoConstraints = false
        optionsScroll.heightAnchor.constraint(equalToConstant: 44).isActive = true

        let grid = NSGridView(views: [
            [label("Name"), name],
            [label("Host"), host],
            [label("User"), user],
            [label("Port"), port],
            [label("Identity file"), identityRow],
            [label("Jump host"), jump],
            [label("Options"), optionsScroll],
            [label("Command"), command],
        ])
        grid.rowSpacing = 6
        grid.columnSpacing = 10
        grid.column(at: 0).xPlacement = .trailing
        grid.column(at: 1).width = 320
        // WITHOUT THIS THE FORM SHIPS BROKEN, and it ships looking half-right, which is
        // worse. The default placement is leading, and a leading-placed cell is sized by
        // its view's intrinsic width. `NSTextField` has one, so the seven plain fields
        // stretch to the 320pt column and look correct. `NSScrollView` and `NSStackView`
        // report `noIntrinsicMetric`, so the Options box and the identity row resolved to
        // ZERO pt wide in v0.28.0 while every gate stayed green (measured 2026-08-14).
        grid.column(at: 1).xPlacement = .fill
        grid.translatesAutoresizingMaskIntoConstraints = false
        addSubview(grid)
        NSLayoutConstraint.activate([
            grid.leadingAnchor.constraint(equalTo: leadingAnchor),
            grid.trailingAnchor.constraint(equalTo: trailingAnchor),
            grid.topAnchor.constraint(equalTo: topAnchor),
            grid.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
        frame.size = fittingSize
    }

    required init?(coder: NSCoder) { nil }

    private func label(_ text: String) -> NSTextField {
        let view = NSTextField(labelWithString: text)
        view.font = NSFont.systemFont(ofSize: 12)
        view.alignment = .right
        return view
    }

    @objc private func chooseIdentity() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        // `~/.ssh` is a dotted directory, so without this the panel opens on a folder the
        // operator cannot see into and the button looks broken.
        panel.showsHiddenFiles = true
        panel.directoryURL = URL(fileURLWithPath: NSString(string: "~/.ssh").expandingTildeInPath)
        panel.message = "Choose a private key. It is referenced by path and never read."
        guard panel.runModal() == .OK, let url = panel.url else { return }
        identity.stringValue = abbreviateHome(url.path)
    }

    /// Writes `~` back over the home directory so a saved block stays portable and reads
    /// the way the operator would have typed it.
    private func abbreviateHome(_ path: String) -> String {
        let home = NSHomeDirectory()
        guard path.hasPrefix(home + "/") else { return path }
        return "~" + path.dropFirst(home.count)
    }

    /// The form's current contents. Trimmed here rather than at every use site, because a
    /// trailing space pasted into the host field is invisible and refuses the connection.
    var connection: SSHConnection {
        var result = SSHConnection()
        result.alias = trimmed(name)
        result.hostName = trimmed(host)
        result.user = trimmed(user)
        result.port = trimmed(port)
        result.identityFile = trimmed(identity)
        result.proxyJump = trimmed(jump)
        result.remoteCommand = trimmed(command)
        // NOT trimmed as a whole: it is multi-line by design, and each line is trimmed and
        // filtered on the way out by `optionList`.
        result.options = options.string
        return result
    }

    /// Fills the form. Exists for the gate, which drives the real fields rather than a
    /// copy of them, so a label swapped onto the wrong field is visible to it.
    func fill(_ connection: SSHConnection) {
        name.stringValue = connection.alias
        host.stringValue = connection.hostName
        user.stringValue = connection.user
        port.stringValue = connection.port
        identity.stringValue = connection.identityFile
        jump.stringValue = connection.proxyJump
        command.stringValue = connection.remoteCommand
        options.string = connection.options
    }

    private func trimmed(_ field: NSTextField) -> String {
        field.stringValue.trimmingCharacters(in: .whitespaces)
    }
}

/// The dialog around the form: title, one explanatory line, the fields, an action row.
///
/// It exists because `NSAlert` was the wrong container and cost two visible defects at
/// once (both measured on the live v0.28.0 window, 2026-08-14):
///
/// - **It re-frames its accessory view**, and inside `NSGridView` that collapsed the two
///   constraint-based cells to 0pt while the seven `NSTextField`s stayed at 320pt. Half
///   the form looked right, which is why it shipped.
/// - **It stacks three action buttons vertically** once their titles do not fit its width,
///   and it reserves 64pt for an app icon nobody needs on a form. The result stood 590pt
///   tall attached to an 816x510 window and hung off the bottom of it.
///
/// Owning the container is what makes the height a fact rather than a hope, and
/// `--smoke-ssh-layout` asserts it against a ceiling derived from the smallest window this
/// host opens.
final class SSHConnectDialog: NSView {
    let form = SSHConnectForm()
    private let note = NSTextField(labelWithString: "")
    /// Set by whichever button ended the sheet. Read once the modal session returns.
    var choice: SSHConnectOutcome = .cancelled

    /// The width of the sheet. The field column is 320pt and the labels want ~80pt, so
    /// anything narrower truncates a label rather than a value, which is the wrong one to
    /// lose.
    static let width: CGFloat = 470
    private static let margin: CGFloat = 18

    init() {
        super.init(frame: NSRect(x: 0, y: 0, width: Self.width, height: 100))

        let title = NSTextField(labelWithString: "New SSH connection")
        title.font = NSFont.systemFont(ofSize: 13, weight: .semibold)

        note.font = NSFont.systemFont(ofSize: 11)
        note.textColor = .secondaryLabelColor
        note.lineBreakMode = .byWordWrapping
        note.maximumNumberOfLines = 2
        showDefaultNote()

        // Cancel sits apart from the two affirmative actions, and the default action is
        // last, which is the platform's reading order for "the one Return performs".
        let cancel = button("Cancel", #selector(cancelled))
        cancel.keyEquivalent = "\u{1b}"
        let connect = button("Connect", #selector(connectOnly))
        let save = button("Save & Connect", #selector(saveAndConnect))
        save.keyEquivalent = "\r"
        // Tinted explicitly rather than left to the default-button look, because that look
        // only appears while the window is KEY. Three identically grey buttons is what the
        // screenshot showed, and a dialog whose primary action is unmarked is one the
        // operator has to read instead of scan.
        save.bezelColor = .controlAccentColor

        let actions = NSStackView(views: [cancel, NSView(), connect, save])
        actions.orientation = .horizontal
        actions.spacing = 8
        // The spacer is the only view allowed to grow, so the buttons keep their natural
        // widths and stay in one row. A stretched button row is how this reads as a
        // web form rather than a mac dialog.
        actions.setHuggingPriority(.defaultLow, for: .horizontal)
        for view in [cancel, connect, save] {
            view.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        }

        let column = NSStackView(views: [title, note, form, actions])
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 12
        column.setCustomSpacing(4, after: title)
        column.translatesAutoresizingMaskIntoConstraints = false
        addSubview(column)

        let margin = Self.margin
        NSLayoutConstraint.activate([
            column.leadingAnchor.constraint(equalTo: leadingAnchor, constant: margin),
            column.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -margin),
            column.topAnchor.constraint(equalTo: topAnchor, constant: margin),
            column.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -margin),
            widthAnchor.constraint(equalToConstant: Self.width),
            form.widthAnchor.constraint(equalTo: column.widthAnchor),
            actions.widthAnchor.constraint(equalTo: column.widthAnchor),
            note.widthAnchor.constraint(equalTo: column.widthAnchor),
        ])
        layoutSubtreeIfNeeded()
        frame.size = NSSize(width: Self.width, height: fittingSize.height)
    }

    required init?(coder: NSCoder) { nil }

    private func button(_ title: String, _ action: Selector) -> NSButton {
        let view = NSButton(title: title, target: self, action: action)
        view.bezelStyle = .rounded
        return view
    }

    func showDefaultNote() {
        note.stringValue = "Saving writes a Host block to ~/.ssh/config, so ssh and scp "
            + "see it too. Nothing is stored inside Mind2t."
        note.textColor = .secondaryLabelColor
    }

    /// Shows why the last attempt was refused, in place of the explanation. Nothing typed
    /// is cleared: throwing away eight fields over one mistyped port is the interaction
    /// this whole dialog exists to avoid.
    func showRefusal(_ why: String) {
        note.stringValue = why
        note.textColor = .systemRed
    }

    private func end(_ outcome: SSHConnectOutcome, _ code: NSApplication.ModalResponse) {
        choice = outcome
        NSApp.stopModal(withCode: code)
    }

    @objc private func cancelled() { end(.cancelled, .cancel) }
    @objc private func connectOnly() { end(.connect(form.connection), .continue) }
    @objc private func saveAndConnect() { end(.saveAndConnect(form.connection), .OK) }
}

enum SSHConnect {
    /// The one place a sheet is built. `prompt`, the layout gate and the screenshot mode
    /// all come through here, so a change to the chrome cannot be true of the shipped
    /// dialog and false of the one the gate measures.
    ///
    /// **No `.titled`.** A titled sheet draws a 32pt title bar with nothing in it, which
    /// is dead space above the first field and 32pt of height the leak ceiling has to pay
    /// for. Caught by looking at `--shot-ssh-dialog`, not by any assertion.
    static func makeSheet(_ dialog: SSHConnectDialog) -> NSWindow {
        let sheet = NSWindow(
            contentRect: dialog.frame, styleMask: [.docModalWindow],
            backing: .buffered, defer: false)
        sheet.contentView = dialog
        sheet.initialFirstResponder = dialog.form.firstField
        sheet.isReleasedWhenClosed = false
        return sheet
    }

    /// The dialog at the size it will actually occupy on screen, with nothing shown. For
    /// `--smoke-ssh-layout`.
    ///
    /// It measures the WINDOW and returns the laid-out root view, and both halves are
    /// deliberate. `SSHConnectForm` measured on its own reported every field at its full
    /// 320pt in the exact build that shipped two 0pt controls, because the collapse
    /// happened when the container re-framed it; and a view-sized ceiling misses whatever
    /// chrome the window adds around it, which was 32pt here.
    static func measureDialog() -> (size: NSSize, root: NSView) {
        let dialog = SSHConnectDialog()
        let sheet = makeSheet(dialog)
        sheet.layoutIfNeeded()
        dialog.layoutSubtreeIfNeeded()
        return (sheet.frame.size, dialog)
    }

    /// Runs the dialog as a sheet on `parent` and returns what the operator chose.
    ///
    /// Loops on a refusal rather than closing, with everything still typed in it and the
    /// reason in red where the explanation was.
    ///
    /// The sheet is run with an explicit modal session rather than `beginSheet`'s
    /// completion handler because the caller is synchronous and the retry loop needs the
    /// answer before it can decide whether to show the sheet again.
    static func prompt(over parent: NSWindow?) -> SSHConnectOutcome {
        let dialog = SSHConnectDialog()
        let sheet = makeSheet(dialog)

        while true {
            let code: NSApplication.ModalResponse
            if let parent {
                parent.beginSheet(sheet)
                code = NSApp.runModal(for: sheet)
                parent.endSheet(sheet)
            } else {
                // No window to hang it on: still modal, just free-standing. This is the
                // path when the + is used before any pane exists.
                sheet.center()
                sheet.makeKeyAndOrderFront(nil)
                code = NSApp.runModal(for: sheet)
                sheet.orderOut(nil)
            }
            if code == .cancel { return .cancelled }

            let connection: SSHConnection
            switch dialog.choice {
            case .cancelled: return .cancelled
            case .connect(let value), .saveAndConnect(let value): connection = value
            }
            if let why = connection.refusal {
                dialog.showRefusal(why)
                continue
            }
            dialog.showDefaultNote()
            return dialog.choice
        }
    }
}
