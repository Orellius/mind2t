// swift-tools-version: 5.9
// The minimal Swift host -- slice 8. Build from this directory (`swift build` resolves the
// -L flag against the invoking directory), or use scripts/build-swift.sh which does both
// halves: the Rust archive first, then this.
import PackageDescription

let package = Package(
    name: "mind2t-host",
    platforms: [.macOS(.v13)],
    targets: [
        // The C surface, imported straight from the crate's own header -- one source of
        // truth, no copied declarations.
        .systemLibrary(name: "CMind2tHost", path: "Sources/CMind2tHost"),
        .executableTarget(
            name: "mind2t-host",
            dependencies: ["CMind2tHost"],
            path: "Sources/mind2t-host",
            linkerSettings: [
                .unsafeFlags(["-L../target/release"]),
                .linkedLibrary("mind2t-vt-host"),
                // wgpu's Metal backend.
                .linkedFramework("Metal"),
                .linkedFramework("QuartzCore"),
                .linkedFramework("AppKit"),
                // S6 panels: the WKWebView that renders them.
                .linkedFramework("WebKit"),
            ]
        ),
    ]
)
