// swift-tools-version:5.3
import PackageDescription
let package = Package(name: "tauri-plugin-mdreader-transfer", platforms: [.iOS(.v15)], products: [
  .library(name: "tauri-plugin-mdreader-transfer", type: .static, targets: ["tauri-plugin-mdreader-transfer"])
], dependencies: [.package(name: "Tauri", path: "../.tauri/tauri-api")], targets: [
  .target(name: "tauri-plugin-mdreader-transfer", dependencies: [.byName(name: "Tauri")], path: "Sources")
])
