// swift-tools-version: 5.10
// PaddockKit holds the models, the protocol layer to the paddock CLI, the
// state stores and the copy of the Mac launcher. It uses Foundation and
// Observation only, so it also builds with a Linux Swift toolchain.
import PackageDescription

let package = Package(
    name: "PaddockKit",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "PaddockKit", targets: ["PaddockKit"]),
    ],
    targets: [
        .target(name: "PaddockKit"),
        // The tests read the shared fixtures in mac/launcher/Fixtures by path.
        .testTarget(name: "PaddockKitTests", dependencies: ["PaddockKit"]),
    ]
)
