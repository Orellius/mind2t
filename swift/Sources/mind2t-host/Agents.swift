// The agent registry, as Swift sees it.
//
// Thin ON PURPOSE. Every fact here comes from `mind2t_agent_*` in the archive: which CLIs
// exist, which of them this machine has, and whether an argv is ours to send. Nothing in this
// file decides any of that.
//
// The one that matters is `screen`. An approval-bypassing flag (--yolo,
// --dangerously-skip-permissions, -a never) is the operator's to type and never ours to add,
// and the guard that enforces it lives in Rust. A Swift copy of that flag table would be a
// second place for the policy to disagree with itself, and the way it would disagree is by
// being out of date -- so this asks rather than knows. The spawn asks again inside the
// archive regardless of what happens here; this call exists so the UI can say WHY before the
// operator hits a wall.

import CMind2tHost
import Foundation

/// One agent CLI, with this machine's answer about it attached.
struct Agent {
    let id: String
    let name: String
    let installHint: String
    /// How long it may sit silent after launch before the silence means failure.
    let spawnGrace: TimeInterval
    /// True for agents that ignore a prompt passed as argv and must be typed into.
    let typeAfterLaunch: Bool
    /// The resolved binary, or nil when it is not installed here.
    let path: String?

    var isInstalled: Bool { path != nil }

    /// The whole registry, each entry resolved against this machine's PATH.
    ///
    /// Rebuilt at every call rather than cached in Swift: the archive already caches (a hit
    /// stays fresh for five minutes, a miss is re-checked after ten seconds), and a second
    /// cache here would be the one that goes stale -- an agent installed mid-session would
    /// never appear until the app restarted.
    static func all() -> [Agent] {
        (0..<mind2t_agent_count()).compactMap { index in
            var info = Mind2tAgentInfo()
            guard mind2t_agent_info(index, &info) == MIND2T_HOST_SUCCESS,
                let id = info.id, let name = info.name, let hint = info.install_hint
            else { return nil }
            let identifier = String(cString: id)
            return Agent(
                id: identifier,
                name: String(cString: name),
                installHint: String(cString: hint),
                spawnGrace: TimeInterval(info.spawn_grace_ms) / 1000,
                typeAfterLaunch: info.type_after_launch,
                path: resolve(identifier))
        }
    }

    /// Where `id`'s binary is, or nil when the machine does not have it.
    ///
    /// IGNORED is the not-installed answer and it is not an error -- that distinction is the
    /// whole reason a menu can show an install hint instead of a dead row.
    static func resolve(_ id: String) -> String? {
        var length = 0
        guard mind2t_agent_resolve(id, nil, 0, &length) == MIND2T_HOST_SUCCESS, length > 0
        else { return nil }
        var buffer = [UInt8](repeating: 0, count: length)
        let filled = buffer.withUnsafeMutableBufferPointer { out in
            mind2t_agent_resolve(id, out.baseAddress, length, &length)
        }
        guard filled == MIND2T_HOST_SUCCESS else { return nil }
        return String(decoding: buffer, as: UTF8.self)
    }

    /// The flag that would turn approvals off and where it sits, or nil when the argv is ours
    /// to send.
    static func screen(_ argv: [String]) -> (flag: String, at: Int)? {
        withCStrings(argv) { pointers in
            var at: UInt32 = 0
            var length = 0
            let sized = mind2t_agent_screen(pointers, argv.count, &at, nil, 0, &length)
            guard sized == MIND2T_HOST_REFUSED else { return nil }
            var buffer = [UInt8](repeating: 0, count: length)
            _ = buffer.withUnsafeMutableBufferPointer { out in
                mind2t_agent_screen(pointers, argv.count, &at, out.baseAddress, length, &length)
            }
            return (String(decoding: buffer, as: UTF8.self), Int(at))
        }
    }
}

/// Why a launch did not happen, in the operator's terms.
enum AgentLaunchFailure: Error {
    /// The argv carried an approval bypass. Refused, never stripped: silently removing a flag
    /// someone typed is worse than refusing it, because they would believe approvals were off
    /// when they were not.
    case refused(flag: String, at: Int)
    /// Not on this machine. Carries what to install.
    case notInstalled(hint: String)
    /// The pty, the renderer, or the child itself.
    case spawnFailed(Mind2tHostResult)

    var summary: String {
        switch self {
        case .refused(let flag, let at):
            return "\(flag) turns this agent's approvals off. Remove it (word \(at + 1)) and "
                + "launch again, or type it yourself in the pane -- that is your call to make, "
                + "not the terminal's."
        case .notInstalled(let hint):
            return "That agent is not installed. Install it with:\n\n\(hint)"
        case .spawnFailed(let result):
            return "The pane could not be opened (result \(result.rawValue))."
        }
    }
}

/// Spawns a registry agent on its own pty, with the same options an ordinary pane gets.
///
/// `cwd` is where the agent starts. The app passes the active session's directory, because
/// "launch Claude" nearly always means "launch it HERE"; nil falls back to the archive's own
/// default, which is HOME.
func spawnAgentHost(
    id: String, argv: [String], cols: UInt16, rows: UInt16, fontSize: Float,
    autoDirection: Bool = false, config: OpaquePointer? = nil, cwd: String? = nil
) -> Result<OpaquePointer, AgentLaunchFailure> {
    var host: OpaquePointer?
    let result = withCStrings(argv) { pointers in
        withOptionalCString(cwd) { cwdPointer in
            var options = Mind2tHostOptions(
                cols: cols, rows: rows, font_size: fontSize,
                // NULL, and it is ignored anyway: the child is the agent, not this.
                command: nil,
                auto_direction: autoDirection, config: config, cwd: cwdPointer)
            return mind2t_host_spawn_agent(&options, id, pointers, argv.count, &host)
        }
    }
    switch result {
    case MIND2T_HOST_SUCCESS:
        guard let host else { return .failure(.spawnFailed(result)) }
        return .success(host)
    case MIND2T_HOST_REFUSED:
        // Asked again here only to REPORT it. The refusal already happened inside the archive
        // and no child was started; this is how the operator learns which word to remove.
        let found = Agent.screen(argv)
        return .failure(.refused(flag: found?.flag ?? "an approval bypass", at: found?.at ?? 0))
    case MIND2T_HOST_IGNORED:
        let hint = Agent.all().first { $0.id == id }?.installHint ?? "see the agent's own docs"
        return .failure(.notInstalled(hint: hint))
    default:
        return .failure(.spawnFailed(result))
    }
}

/// Lends `strings` to `body` as a C argv for the duration of the call.
///
/// Nested `withCString` rather than a loop collecting pointers: a `[UnsafePointer]` assembled
/// from temporaries dangles the moment each temporary dies, and the resulting garbage argv is
/// the kind of bug that works in a debug build and corrupts a flag in a release one.
func withCStrings<T>(
    _ strings: [String], _ body: (UnsafePointer<UnsafePointer<CChar>?>?) -> T
) -> T {
    func step(_ index: Int, _ built: [UnsafePointer<CChar>?]) -> T {
        if index == strings.count {
            if built.isEmpty { return body(nil) }
            return built.withUnsafeBufferPointer { body($0.baseAddress) }
        }
        return strings[index].withCString { pointer in
            step(index + 1, built + [pointer])
        }
    }
    return step(0, [])
}
