// swift-tools-version:5.5
// filebridge's iOS plugin (src/native.rs), built and linked by build.rs.

import PackageDescription

let package = Package(
  name: "filebridge-native",
  platforms: [
    .iOS(.v15)
  ],
  products: [
    .library(
      name: "filebridge-native",
      type: .static,
      targets: ["filebridge-native"])
  ],
  dependencies: [
    // Copied here by build.rs from the tauri crate, as tauri-plugin does for plugin crates.
    .package(name: "Tauri", path: "../.tauri/tauri-api")
  ],
  targets: [
    .target(
      name: "filebridge-native",
      dependencies: [
        .byName(name: "Tauri")
      ],
      path: "Sources")
  ]
)
