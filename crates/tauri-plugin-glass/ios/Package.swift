// swift-tools-version:5.9
// Design 198: the upstream Tauri plugin package shape (tauri-plugin-haptics' ios/Package.swift) with tools 5.9 so
// the platform floor can say iOS 17 (the app's deployment target is 17.4): the overlay uses iOS 17 UIKit
// (`registerForTraitChanges`, `keyboardLayoutGuide.usesBottomSafeArea`) without availability ladders. Glass itself
// is iOS 26, behind `#if compiler(>=6.2)` + `#available(iOS 26.0, *)` in the sources.
// The library name must equal the crate name: swift-rs links it as `tauri-plugin-glass`.

import PackageDescription

let package = Package(
  name: "tauri-plugin-glass",
  platforms: [
    .macOS(.v10_13),
    .iOS(.v17),
  ],
  products: [
    .library(
      name: "tauri-plugin-glass",
      type: .static,
      targets: ["tauri-plugin-glass"])
  ],
  dependencies: [
    // Copied by tauri-plugin's build script (`build.rs` → `.tauri/tauri-api`); the package identity is the folder name.
    .package(path: "../.tauri/tauri-api")
  ],
  targets: [
    .target(
      name: "tauri-plugin-glass",
      dependencies: [
        .product(name: "Tauri", package: "tauri-api")
      ],
      path: "Sources")
  ]
)
