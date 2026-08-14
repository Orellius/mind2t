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
        let identityRow = NSStackView(views: [identity, browse])
        identityRow.orientation = .horizontal
        identityRow.spacing = 6

        options.font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        options.isRichText = false
        options.isAutomaticQuoteSubstitutionEnabled = false
        let optionsScroll = NSScrollView()
        optionsScroll.documentView = options
        optionsScroll.hasVerticalScroller = true
        optionsScroll.borderType = .bezelBorder
        optionsScroll.translatesAutoresizingMaskIntoConstraints = false
        optionsScroll.heightAnchor.constraint(equalToConstant: 54).isActive = true

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
        grid.rowSpacing = 8
        grid.columnSpacing = 10
        grid.column(at: 0).xPlacement = .trailing
        grid.column(at: 1).width = 320
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

enum SSHConnect {
    /// Runs the form modally and returns what the operator chose.
    ///
    /// Loops on a refusal rather than closing: the form comes back with everything still
    /// typed in it and the reason above the fields. Throwing away eight fields because one
    /// port was mistyped is the interaction this is avoiding.
    static func prompt() -> SSHConnectOutcome {
        let form = SSHConnectForm()
        var problem: String?
        while true {
            let alert = NSAlert()
            alert.messageText = "New SSH connection"
            alert.informativeText =
                problem
                ?? "Saving writes a Host block to ~/.ssh/config, so ssh, scp and every other "
                    + "tool on this machine sees it too. Nothing is stored inside Mind2t."
            if problem != nil { alert.alertStyle = .warning }
            alert.accessoryView = form
            alert.addButton(withTitle: "Save & Connect")
            alert.addButton(withTitle: "Connect")
            alert.addButton(withTitle: "Cancel")
            alert.window.initialFirstResponder = form.firstField

            let choice = alert.runModal()
            if choice == .alertThirdButtonReturn { return .cancelled }

            let connection = form.connection
            if let why = connection.refusal {
                problem = why
                continue
            }
            return choice == .alertFirstButtonReturn
                ? .saveAndConnect(connection) : .connect(connection)
        }
    }
}
