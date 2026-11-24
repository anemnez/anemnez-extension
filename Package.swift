// swift-tools-version: 6.0
import PackageDescription

// The contract's Swift client (ERGON-UI.md §4a; M m/33097, m/33149): AnemnezEvents, the Swift sibling of
// anemnez-extension-client, which every face takes to read a product's Events face. It's at the repo root because
// SwiftPM takes a git dependency only from a root Package.swift; the sources stay in swift/, and Cargo ignores this
// file. Its protos are links to the crate's own, never copies. The events' own bytes decode through the Rust codec
// (hydramq-message-ffi, A's) as an xcframework, never a second codec (D m/33118).
let package = Package(
    name: "AnemnezExtension",
    // grpc-swift 2 is available from macOS 15, iOS 18 and visionOS 2: every face's platform.
    platforms: [.macOS(.v15), .iOS(.v18), .visionOS(.v2)],
    products: [
        .library(name: "AnemnezEvents", targets: ["AnemnezEvents"]),
    ],
    dependencies: [
        // The same pins as anemnez-service's Swift package, so a face resolves one copy of each.
        .package(url: "https://github.com/grpc/grpc-swift-2.git", exact: "2.4.4"),
        .package(url: "https://github.com/grpc/grpc-swift-nio-transport.git", exact: "2.12.0"),
        .package(url: "https://github.com/grpc/grpc-swift-protobuf.git", exact: "2.5.0"),
        .package(url: "https://github.com/apple/swift-protobuf.git", exact: "1.39.0"),
    ],
    targets: [
        .target(
            name: "AnemnezEvents",
            dependencies: [
                .product(name: "GRPCCore", package: "grpc-swift-2"),
                .product(name: "GRPCNIOTransportHTTP2", package: "grpc-swift-nio-transport"),
                .product(name: "GRPCProtobuf", package: "grpc-swift-protobuf"),
                .product(name: "SwiftProtobuf", package: "swift-protobuf"),
            ],
            path: "swift/Sources/AnemnezEvents",
            plugins: [
                .plugin(name: "GRPCProtobufGenerator", package: "grpc-swift-protobuf"),
            ]
        ),
        .testTarget(
            name: "AnemnezEventsTests",
            dependencies: ["AnemnezEvents"],
            path: "swift/Tests/AnemnezEventsTests"
        ),
    ]
)
