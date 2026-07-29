// swift-tools-version: 5.9
// The minimal Swift host -- slice 8. Build from this directory (`swift build` resolves the
// -L flag against the invoking directory), or use scripts/build-swift.sh which does both
// halves: the Rust archive first, then this.
import PackageDescription

let package = Package(
    name: "ruuah-host",
    platforms: [.macOS(.v13)],
    targets: [
        // The C surface, imported straight from the crate's own header -- one source of
        // truth, no copied declarations.
        .systemLibrary(name: "CRuuahHost", path: "Sources/CRuuahHost"),
        .executableTarget(
            name: "ruuah-host",
            dependencies: ["CRuuahHost"],
            path: "Sources/ruuah-host",
            linkerSettings: [
                .unsafeFlags(["-L../target/release"]),
                .linkedLibrary("ruuah-vt-host"),
                // wgpu's Metal backend.
                .linkedFramework("Metal"),
                .linkedFramework("QuartzCore"),
                .linkedFramework("AppKit"),
            ]
        ),
    ]
)
