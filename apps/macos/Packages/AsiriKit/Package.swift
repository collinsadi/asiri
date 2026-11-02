// swift-tools-version: 5.10
// AsiriKit: everything in the Mac app except the Rust core (architecture §6.2).
//
//   AsiriDesign    colors, type, bubbles, avatars, wallpaper, motion
//   AsiriDomain    the app's models and the CoreService protocol, plus PreviewCore
//
// The real core comes from the AsiriCoreBridge package, so these targets build without
// compiling Rust.
import PackageDescription

let package = Package(
    name: "AsiriKit",
    defaultLocalization: "en",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "AsiriDomain", targets: ["AsiriDomain"]),
    ],
    targets: [
        .target(name: "AsiriDomain"),
        .target(name: "AsiriDesign", dependencies: ["AsiriDomain"]),
    ]
)
