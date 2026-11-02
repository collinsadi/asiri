// swift-tools-version: 5.10
// AsiriCoreBridge: the Rust core (AsiriCoreFFI.xcframework), its generated Swift bindings
// (AsiriCoreKit) and LiveCore, which implements AsiriDomain's CoreService with them.
//
// Build the XCFramework and bindings first: tools/release/build-xcframework.sh
import PackageDescription

let package = Package(
    name: "AsiriCoreBridge",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "AsiriCoreBridge", targets: ["AsiriBridge"]),
    ],
    dependencies: [
        .package(path: "../AsiriKit"),
    ],
    targets: [
        .binaryTarget(name: "AsiriCoreFFI", path: "../../Frameworks/AsiriCoreFFI.xcframework"),
        .target(
            name: "AsiriCoreKit",
            dependencies: ["AsiriCoreFFI"],
            linkerSettings: [
                // What the Rust core needs from the system: SQLCipher (CommonCrypto, Security),
                // Arti (SystemConfiguration, resolver).
                .linkedFramework("Security"),
                .linkedFramework("SystemConfiguration"),
                .linkedFramework("CoreFoundation"),
                .linkedLibrary("resolv"),
                .linkedLibrary("iconv"),
                .linkedLibrary("c++"),
            ]
        ),
        .target(name: "AsiriBridge", dependencies: ["AsiriCoreKit", .product(name: "AsiriDomain", package: "AsiriKit")]),
    ]
)
