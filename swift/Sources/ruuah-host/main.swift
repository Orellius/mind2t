// The minimal Swift host, slice 8. Two modes:
//
//   ruuah-host --smoke        headless: spawn a known child, poll until its pixels arrive,
//                             assert there is ink, exit 0/1. This is the CI-assertable
//                             proof that the ABI is consumable from Swift at all.
//   ruuah-host [--command X]  a window: blit polled frames, forward keys, live resize.
//
// Everything the host knows arrives through CRuuahHost -- the five ruuah_host_* calls.
// There is deliberately no second channel into the Rust side.

import AppKit
import CRuuahHost

func spawnHost(
    cols: UInt16, rows: UInt16, fontSize: Float, command: String?, autoDirection: Bool = false,
    config: OpaquePointer? = nil
) -> OpaquePointer? {
    var host: OpaquePointer?
    let result: RuuahHostResult
    if let command {
        result = command.withCString { pointer in
            var options = RuuahHostOptions(
                cols: cols, rows: rows, font_size: fontSize, command: pointer,
                auto_direction: autoDirection, config: config)
            return ruuah_host_spawn(&options, &host)
        }
    } else {
        var options = RuuahHostOptions(
            cols: cols, rows: rows, font_size: fontSize, command: nil,
            auto_direction: autoDirection, config: config)
        result = ruuah_host_spawn(&options, &host)
    }
    guard result == RUUAH_HOST_SUCCESS, let host else {
        FileHandle.standardError.write(Data("spawn failed: \(result)\n".utf8))
        return nil
    }
    return host
}

/// Headless proof: a child's output becomes ink, through the archive, from Swift.
func runSmoke() -> Int32 {
    guard let host = spawnHost(
        cols: 80, rows: 24, fontSize: 0, command: "printf 'RUUAH-VT-SMOKE\\n'")
    else { return 1 }
    defer { ruuah_host_free(host) }

    let deadline = Date().addingTimeInterval(10)
    var frame = RuuahHostFrame()
    while Date() < deadline {
        guard ruuah_host_poll(host, &frame) == RUUAH_HOST_SUCCESS else {
            FileHandle.standardError.write(Data("poll failed\n".utf8))
            return 1
        }
        if frame.child_exited, let pixels = frame.pixels, !frame.drew {
            // The child is gone and its final frame is drawn. Ink = any pixel that is not
            // the background, and the background is whatever the top-left corner shows.
            let count = Int(frame.width) * Int(frame.height)
            var ink = 0
            for index in 0..<count {
                let pixel = pixels.advanced(by: index * 4)
                if pixel[0] != pixels[0] || pixel[1] != pixels[1] || pixel[2] != pixels[2] {
                    ink += 1
                }
            }
            guard ink > 0 else {
                FileHandle.standardError.write(Data("frame arrived with no ink\n".utf8))
                return 1
            }
            print("SMOKE OK: \(ink) ink pixels in \(frame.width)x\(frame.height), generation \(frame.generation)")
            return 0
        }
        usleep(10_000)
    }
    FileHandle.standardError.write(Data("no settled frame within 10s\n".utf8))
    return 1
}

let arguments = CommandLine.arguments
if arguments.contains("--smoke") {
    exit(runSmoke())
}

var command: String?
if let index = arguments.firstIndex(of: "--command"), index + 1 < arguments.count {
    command = arguments[index + 1]
}

// Settings (S1): ~/.ruuah/config.toml, or --config-dir for a capture/test that must not
// touch the real one. Loading never fails into an unusable state; anything that could
// not be honoured arrives as an error string the app shows loudly on launch.
var configDir: String?
if let index = arguments.firstIndex(of: "--config-dir"), index + 1 < arguments.count {
    configDir = arguments[index + 1]
}
var config: OpaquePointer?
if let configDir {
    _ = configDir.withCString { pointer in ruuah_config_load(pointer, &config) }
} else {
    _ = ruuah_config_load(nil, &config)
}
let configError = ruuah_config_error(config).map { String(cString: $0) }

// When the binary runs from an assembled .app (scripts/build-app.sh), the app is
// Hebrew-first: auto base direction unless --ltr. The bare CLI binary keeps the
// flag-driven defaults unchanged. A window opens straight into the login shell --
// no splash, like Ghostty; the bundled banner remains available via --splash.
let bundledBanner = Bundle.main.path(forResource: "banner", ofType: "sh")
if command == nil, arguments.contains("--splash"), let bundledBanner {
    command = "sh '\(bundledBanner)'"
}
// The config's shell is the default for new sessions; explicit CLI intent outranks it.
if command == nil, !arguments.contains("--splash"),
    let shell = ruuah_config_shell(config).map({ String(cString: $0) })
{
    command = shell
}
// CLI flags outrank the config, which outranks the bundle default (the .app is
// Hebrew-first; the bare CLI binary defaults LTR).
let autoDirection: Bool
if arguments.contains("--auto-direction") {
    autoDirection = true
} else if arguments.contains("--ltr") {
    autoDirection = false
} else {
    autoDirection = ruuah_config_auto_direction(config, bundledBanner != nil)
}
// Logical size; the delegate multiplies by the backing scale at spawn.
let configFontSize = ruuah_config_font_size(config)
let baseFontSize: Float = configFontSize > 0 ? configFontSize : 16

// Shell integration (S2): spawned shells inherit this process's environment, so pointing
// ZDOTDIR at the bundled bootstrap is all the wiring blocks need. zsh-only by nature (the
// variable means nothing to other shells), .app-only in practice (the bare CLI binary has
// no resource bundle, and an externally-set ZDOTDIR chain is left alone there).
if let resources = Bundle.main.resourcePath {
    let zdotdir = resources + "/shell/zdotdir"
    let integration = resources + "/shell/ruuah-integration.zsh"
    if FileManager.default.fileExists(atPath: zdotdir + "/.zshenv"),
        FileManager.default.fileExists(atPath: integration)
    {
        setenv("RUUAH_INTEGRATION", integration, 1)
        if let original = getenv("ZDOTDIR") {
            setenv("RUUAH_USER_ZDOTDIR", String(cString: original), 1)
        }
        setenv("ZDOTDIR", zdotdir, 1)
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let delegate = HostAppDelegate(
    command: command, autoDirection: autoDirection, config: config,
    baseFontSize: baseFontSize, configError: configError)
app.delegate = delegate
app.run()
