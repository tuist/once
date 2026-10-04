// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "VaporGraph",
    platforms: [.macOS(.v13)],
    products: [
        .library(name: "Vapor", targets: ["Vapor"]),
        .library(name: "XCTVapor", targets: ["XCTVapor"]),
        .library(name: "VaporTesting", targets: ["VaporTesting"]),
    ],
    dependencies: [
        .package(path: "Dependencies/swift-nio"),
        .package(path: "Dependencies/swift-log"),
    ],
    targets: [
        .target(name: "CVaporBcrypt"),
        .target(name: "Vapor", dependencies: [
            "CVaporBcrypt",
            .product(name: "NIO", package: "swift-nio"),
            .product(name: "Logging", package: "swift-log"),
            .product(name: "NIOTransportServices", package: "swift-nio", condition: .when(platforms: [.iOS])),
        ], exclude: ["Excluded.swift"], swiftSettings: [.define("GRAPH_FIXTURE")]),
        .executableTarget(name: "Development", dependencies: ["Vapor"]),
        .target(name: "VaporTestUtils", dependencies: ["Vapor"]),
        .target(name: "XCTVapor", dependencies: ["VaporTestUtils", "Vapor"]),
        .target(name: "VaporTesting", dependencies: ["VaporTestUtils", "Vapor"]),
        .testTarget(name: "VaporTests", dependencies: [
            "XCTVapor", "VaporTesting", "Vapor",
            .product(name: "NIOTestUtils", package: "swift-nio"),
        ], resources: [.copy("Resources/file with spaces.txt")]),
    ]
)
