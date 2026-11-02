// swift-tools-version: 5.10
// AsiriCalls: call media through the LiveKit Swift SDK, with frames encrypted end to end by the
// key the core derives from the chat's MLS group (architecture §10.5).
//
// LiveKit is pinned to an exact release; bump it deliberately, after reading its changelog.
import PackageDescription

let package = Package(
    name: "AsiriCalls",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "AsiriCalls", targets: ["AsiriCalls"]),
    ],
    dependencies: [
        .package(path: "../AsiriKit"),
        .package(url: "https://github.com/livekit/client-sdk-swift.git", exact: "2.18.0"),
    ],
    targets: [
        .target(
            name: "AsiriCalls",
            dependencies: [
                .product(name: "AsiriKit", package: "AsiriKit"),
                .product(name: "LiveKit", package: "client-sdk-swift"),
            ]
        ),
    ]
)
