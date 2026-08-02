// The web panel seam: a WKWebView hosting a locally built React document, with a typed
// message in each direction and nothing else crossing between them.
//
// Why a webview at all, when the terminal itself is native down to the pixel: the panels
// this serves are documents, not grids. A diff with syntax colouring, a markdown preview,
// a settings form -- these are where AppKit costs days and a browser engine costs hours,
// and none of them is on the input-latency path. The rule that keeps this honest is that
// THE TERMINAL SURFACE NEVER ENTERS THE WEBVIEW: no terminal pixels, no keystrokes on
// their way to a pty, no frame data. The panel is a sibling of the grid, never a layer
// over it.
//
// Everything here is off unless `panels = true` in config.toml (ruuah_config_panels).

import AppKit
import WebKit

// MARK: - the wire

/// A failure with a stated reason.
///
/// Every failure on this seam ends up in one of two places -- a message shown in the
/// panel, or a line on stderr -- so the reason IS the payload and there is nothing to
/// gain from an enum of cases nobody switches on. It exists because `Result`'s failure
/// type must be an `Error`, and because a bare `String` there would let a reason be
/// silently dropped at a call site that only checks for nil.
struct PanelFailure: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = description
    }
}

/// Host to panel. Mirrors `web/src/protocol.ts`; the two are edited together.
enum HostMessage {
    case initialize(theme: PanelTheme)
    case files(repo: String?, files: [ChangedFile], error: String?)
    case fileDiff(path: String, patch: String, error: String?)
    /// Liveness probe. The panel answers from its bridge module, not its UI, so a
    /// pong proves the CHANNEL rather than whatever happens to be rendered.
    case ping(nonce: String)

    var json: [String: Any] {
        switch self {
        case .initialize(let theme):
            return ["kind": "init", "theme": theme.json]
        case .files(let repo, let files, let error):
            return [
                "kind": "files", "repo": repo ?? NSNull(),
                "files": files.map(\.json), "error": error ?? NSNull(),
            ]
        case .fileDiff(let path, let patch, let error):
            return ["kind": "fileDiff", "path": path, "patch": patch, "error": error ?? NSNull()]
        case .ping(let nonce):
            return ["kind": "ping", "nonce": nonce]
        }
    }
}

struct PanelTheme {
    let background: String
    let foreground: String
    let accent: String
    let dim: String

    var json: [String: Any] {
        ["background": background, "foreground": foreground, "accent": accent, "dim": dim]
    }
}

struct ChangedFile {
    let path: String
    let status: String
    let additions: Int
    let deletions: Int

    var json: [String: Any] {
        ["path": path, "status": status, "additions": additions, "deletions": deletions]
    }
}

/// Panel to host, after validation. An unparseable body never becomes one of these.
enum PanelMessage: Equatable {
    case ready
    case refresh
    case dismiss
    case requestDiff(path: String)
    case pong(nonce: String)
    case decodeError(detail: String)

    /// Decodes a raw script-message body.
    ///
    /// Returns the reason on failure rather than nil, because "the panel sent something
    /// I could not read" is a bug worth naming and a silent nil is how it would hide.
    static func decode(_ body: Any) -> Result<PanelMessage, PanelFailure> {
        guard let object = body as? [String: Any] else {
            return .failure(PanelFailure("message body is \(type(of: body)), expected an object"))
        }
        guard let kind = object["kind"] as? String else {
            return .failure(PanelFailure("message.kind missing or not a string"))
        }
        switch kind {
        case "ready": return .success(PanelMessage.ready)
        case "refresh": return .success(PanelMessage.refresh)
        case "dismiss": return .success(PanelMessage.dismiss)
        case "requestDiff":
            guard let path = object["path"] as? String else {
                return .failure(PanelFailure("requestDiff.path missing or not a string"))
            }
            return .success(PanelMessage.requestDiff(path: path))
        case "pong":
            guard let nonce = object["nonce"] as? String else {
                return .failure(PanelFailure("pong.nonce missing or not a string"))
            }
            return .success(PanelMessage.pong(nonce: nonce))
        case "decodeError":
            guard let detail = object["detail"] as? String else {
                return .failure(PanelFailure("decodeError.detail missing or not a string"))
            }
            return .success(PanelMessage.decodeError(detail: detail))
        default:
            return .failure(PanelFailure("unknown message kind \"\(kind)\""))
        }
    }
}

// MARK: - the view

final class WebPanel: NSView, WKScriptMessageHandler, WKNavigationDelegate {
    /// Every validated message from the panel. Decode failures never reach here; they
    /// go to `onProtocolError` so a broken bridge is loud instead of merely inert.
    var onMessage: ((PanelMessage) -> Void)?
    var onProtocolError: ((String) -> Void)?

    private let webView: WKWebView
    /// The document we loaded. Any navigation away from it is refused.
    private let documentURL: URL
    /// The same document as a resolved filesystem path, which is what the policy check
    /// compares against.
    ///
    /// URL equality is the wrong test and it fails CLOSED, which is the worst way for a
    /// security check to be wrong: WKWebView hands the delegate a standardized,
    /// percent-encoded URL, and on this machine `/tmp` resolves through a symlink to
    /// `/private/tmp`, so the document we ourselves loaded compared unequal to itself
    /// and the panel refused to display anything at all (caught by --smoke-panel,
    /// 2026-08-02).
    private let documentPath: String
    /// Whether the panel has announced itself. NOT the same as the document having
    /// loaded.
    ///
    /// The bundle is an ES module, and module scripts are deferred: WKWebView's
    /// `didFinish` can fire BEFORE the module has evaluated, so at that moment
    /// `window.__ruuahReceive` may not exist yet and delivering to it throws
    /// "undefined is not a function". Flushing on the panel's own `ready` message
    /// removes the race by construction -- the panel cannot send it before its bridge
    /// module ran, because the bridge is what sends it. (Caught by --smoke-panel,
    /// 2026-08-02; the load-event version worked by timing and would have failed on a
    /// slower machine or a larger bundle.)
    private var ready = false
    /// Messages posted before the panel announced itself, sent in order once it has.
    private var queued: [HostMessage] = []

    /// The built panel document, or nil when it is not on disk.
    ///
    /// Bundle Resources in the assembled .app; `--web-dir` (or the repo's own
    /// `web/dist`) for the bare CLI binary, which has no resource bundle -- the same
    /// escape hatch `--config-dir` already provides, and what lets the headless smoke
    /// run against a freshly built bundle.
    static func documentURL(override: String?) -> URL? {
        let candidates: [String]
        if let override {
            candidates = [(override as NSString).appendingPathComponent("index.html")]
        } else if let resource = Bundle.main.resourcePath {
            candidates = [resource + "/web/index.html"]
        } else {
            candidates = []
        }
        for path in candidates where FileManager.default.fileExists(atPath: path) {
            return URL(fileURLWithPath: path)
        }
        return nil
    }

    init?(documentURL: URL) {
        self.documentURL = documentURL
        self.documentPath = documentURL.resolvingSymlinksInPath().standardizedFileURL.path

        let configuration = WKWebViewConfiguration()
        // Nothing here is a browser session: no cookies, no cache, no local storage
        // surviving the panel being closed.
        configuration.websiteDataStore = .nonPersistent()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        let controller = WKUserContentController()
        configuration.userContentController = controller

        webView = WKWebView(frame: .zero, configuration: configuration)
        super.init(frame: .zero)

        controller.add(self, name: "ruuah")
        webView.navigationDelegate = self
        webView.translatesAutoresizingMaskIntoConstraints = false
        // Opaque on purpose. Transparency behind a WKWebView needs a KVC poke at a
        // private `drawsBackground`, and the card's rounded corners and border are the
        // container's job anyway -- so there is no reason to reach for it.
        webView.allowsBackForwardNavigationGestures = false
        // Web Inspector only when explicitly asked for. It is how the panel gets
        // debugged during development and it has no business being reachable otherwise.
        if #available(macOS 13.3, *),
            ProcessInfo.processInfo.environment["RUUAH_PANEL_INSPECT"] == "1"
        {
            webView.isInspectable = true
        }

        wantsLayer = true
        layer?.cornerRadius = 12
        layer?.masksToBounds = true
        layer?.borderWidth = 1
        layer?.borderColor = NSColor(white: 1, alpha: 0.14).cgColor

        addSubview(webView)
        NSLayoutConstraint.activate([
            webView.leadingAnchor.constraint(equalTo: leadingAnchor),
            webView.trailingAnchor.constraint(equalTo: trailingAnchor),
            webView.topAnchor.constraint(equalTo: topAnchor),
            webView.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])

        webView.loadFileURL(documentURL, allowingReadAccessTo: documentURL)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    deinit {
        webView.configuration.userContentController.removeScriptMessageHandler(forName: "ruuah")
    }

    /// Takes keyboard focus, so Escape and the j/k navigation reach the document.
    func focus() {
        window?.makeFirstResponder(webView)
    }

    /// Sends one message to the panel, queueing until the document can receive it.
    func post(_ message: HostMessage) {
        guard ready else {
            queued.append(message)
            return
        }
        guard let data = try? JSONSerialization.data(withJSONObject: message.json),
            let json = String(data: data, encoding: .utf8)
        else {
            onProtocolError?("host message could not be serialized: \(message.json["kind"] ?? "?")")
            return
        }
        // JSON is a JS expression, with one exception worth pre-empting: U+2028/U+2029
        // are legal inside JSON strings and were line terminators in JS before ES2019.
        // Escaping them costs nothing and removes the question.
        let safe = json
            .replacingOccurrences(of: "\u{2028}", with: "\\u2028")
            .replacingOccurrences(of: "\u{2029}", with: "\\u2029")
        webView.evaluateJavaScript("window.__ruuahReceive(\(safe))") { [weak self] _, error in
            if let error {
                self?.onProtocolError?("delivering \(message.json["kind"] ?? "?"): \(error)")
            }
        }
    }

    // MARK: WKNavigationDelegate

    /// Deliberately does NOT release the queue -- see `ready`. A finished navigation
    /// means the document parsed, not that its module ran.
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {}

    func webView(
        _ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error
    ) {
        onProtocolError?("panel document failed to load: \(error.localizedDescription)")
    }

    func webView(
        _ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!,
        withError error: Error
    ) {
        onProtocolError?("panel document failed to load: \(error.localizedDescription)")
    }

    /// The panel is one local document and stays that way.
    ///
    /// Without this, a link in rendered content -- a path in a diff, a URL in a commit
    /// message -- could navigate the panel somewhere, and a webview that can leave its
    /// document is a webview that can load a remote origin inside the terminal's
    /// process. Nothing legitimate needs it: the document has no links it should follow
    /// itself, and opening a URL for the user is the host's job, in the user's browser.
    func webView(
        _ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
        decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
    ) {
        let target = navigationAction.request.url
        let targetPath = target?.isFileURL == true
            ? target?.resolvingSymlinksInPath().standardizedFileURL.path
            : nil
        if navigationAction.navigationType == .other, targetPath == documentPath {
            decisionHandler(.allow)
            return
        }
        onProtocolError?(
            "refused navigation to \(navigationAction.request.url?.absoluteString ?? "?")")
        decisionHandler(.cancel)
    }

    // MARK: WKScriptMessageHandler

    func userContentController(
        _ controller: WKUserContentController, didReceive message: WKScriptMessage
    ) {
        switch PanelMessage.decode(message.body) {
        case .success(let decoded):
            if case .ready = decoded, !ready {
                ready = true
                let pending = queued
                queued.removeAll()
                for queuedMessage in pending {
                    post(queuedMessage)
                }
            }
            onMessage?(decoded)
        case .failure(let reason):
            onProtocolError?(reason.description)
        }
    }
}
