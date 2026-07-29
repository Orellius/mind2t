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
    cols: UInt16, rows: UInt16, fontSize: Float, command: String?, autoDirection: Bool = false
) -> OpaquePointer? {
    var host: OpaquePointer?
    let result: RuuahHostResult
    if let command {
        result = command.withCString { pointer in
            var options = RuuahHostOptions(
                cols: cols, rows: rows, font_size: fontSize, command: pointer,
                auto_direction: autoDirection)
            return ruuah_host_spawn(&options, &host)
        }
    } else {
        var options = RuuahHostOptions(
            cols: cols, rows: rows, font_size: fontSize, command: nil,
            auto_direction: autoDirection)
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

// When the binary runs from an assembled .app (scripts/build-app.sh), the bundle carries
// the RUUAH splash and the app is Hebrew-first: auto base direction unless --ltr. The
// bare CLI binary has no such resource and keeps the flag-driven defaults unchanged.
let bundledBanner = Bundle.main.path(forResource: "banner", ofType: "sh")
if command == nil, let bundledBanner {
    command = "sh '\(bundledBanner)'"
}
let autoDirection =
    arguments.contains("--auto-direction")
    || (bundledBanner != nil && !arguments.contains("--ltr"))

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let delegate = HostAppDelegate(command: command, autoDirection: autoDirection)
app.delegate = delegate
app.run()
