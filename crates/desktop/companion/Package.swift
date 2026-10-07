// swift-tools-version: 6.0
// Spike SA: `.macOS(.v15)` needs PackageDescription 6.0; Swift 5 language mode keeps the AppKit
// top-level code free of strict-concurrency annotations.
import PackageDescription

let swift5: [SwiftSetting] = [.swiftLanguageMode(.v5)]

let package = Package(
    name: "TheuthCompanion",
    platforms: [.macOS(.v15)],
    products: [
        .library(name: "TheuthKit", targets: ["TheuthKit"]),
        .library(name: "CompanionCore", targets: ["CompanionCore"]),
        .executable(name: "Companion", targets: ["Companion"]),
    ],
    dependencies: [
        // 1.10.0 is the last tag without `#Preview` blocks: the Command Line Tools ship no PreviewsMacros
        // plugin, so every 2.x tag fails to compile outside Xcode. The API used (`Name`, `onKeyUp`) is identical.
        .package(url: "https://github.com/sindresorhus/KeyboardShortcuts", exact: "1.10.0"),
    ],
    targets: [
        .target(name: "TheuthKit", swiftSettings: swift5),
        .target(name: "CompanionCore", dependencies: ["TheuthKit"], swiftSettings: swift5),
        .executableTarget(
            name: "Companion",
            dependencies: [
                "TheuthKit",
                "CompanionCore",
                .product(name: "KeyboardShortcuts", package: "KeyboardShortcuts"),
            ],
            swiftSettings: swift5
        ),
        .testTarget(
            name: "TheuthKitTests",
            dependencies: ["TheuthKit"],
            resources: [.copy("Fixtures")],
            swiftSettings: swift5
        ),
        .testTarget(name: "CompanionCoreTests", dependencies: ["CompanionCore"], swiftSettings: swift5),
    ]
)
