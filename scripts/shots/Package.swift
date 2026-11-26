// swift-tools-version: 6.0
import PackageDescription

// The tests of the Mac harness's own code. An app compiles the harness's
// sources from the folder `builds shots harness mac` writes, never through
// this package. Run them with `swift test --package-path scripts/shots`:
// they add virtual displays, so they need a Mac with a screen to add one to.
let package = Package(
    name: "ShotHarness",
    platforms: [.macOS(.v15)],
    targets: [
        .target(
            name: "ShotHarness",
            path: "harness/mac",
            // The display keeper, which `shot-display.py` compiles by itself.
            exclude: ["display"]
        ),
        .testTarget(
            name: "ShotHarnessTests",
            dependencies: ["ShotHarness"],
            path: "harness-tests/mac"
        ),
    ]
)
