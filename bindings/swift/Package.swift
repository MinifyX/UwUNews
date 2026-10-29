// swift-tools-version:6.2
import PackageDescription

// The Rust core as scripts/build-apple.sh builds it, and the Swift that UniFFI generates for it.
let package = Package(
    name: "UwUNewsCore",
    platforms: [.iOS(.v26)],
    products: [
        .library(name: "UwUNewsCore", targets: ["UwUNewsCore"]),
    ],
    targets: [
        .binaryTarget(name: "UwUNewsCoreFFI", path: "build/UwUNewsCoreFFI.xcframework"),
        .target(
            name: "UwUNewsCore",
            dependencies: ["UwUNewsCoreFFI"],
            // Generated code: written for Swift 5's checks.
            swiftSettings: [.swiftLanguageMode(.v5)],
            linkerSettings: [
                .linkedFramework("Security"),
                .linkedFramework("CoreFoundation"),
                .linkedFramework("SystemConfiguration"),
            ]
        ),
    ]
)
