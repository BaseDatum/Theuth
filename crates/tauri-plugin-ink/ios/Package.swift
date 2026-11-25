// swift-tools-version:5.9
// 218 §6: the whiteboard's native Pencil layer. Built by `tauri-plugin`'s build script for the iOS
// targets and linked into the app (`init_plugin_ink`).

import PackageDescription

let package = Package(
  name: "tauri-plugin-ink",
  platforms: [
    .iOS(.v17)
  ],
  products: [
    .library(
      name: "tauri-plugin-ink",
      type: .static,
      targets: ["tauri-plugin-ink"])
  ],
  dependencies: [
    .package(name: "Tauri", path: "../.tauri/tauri-api")
  ],
  targets: [
    .target(
      name: "tauri-plugin-ink",
      dependencies: [
        .byName(name: "Tauri")
      ],
      path: "Sources",
      linkerSettings: [
        .linkedFramework("PencilKit"),
        .linkedFramework("Vision"),
      ])
  ]
)
