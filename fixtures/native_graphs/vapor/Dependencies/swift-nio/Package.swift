// swift-tools-version: 6.0
import PackageDescription
let package = Package(name: "NetworkingPackage", products: [
    .library(name: "NIO", targets: ["NIO", "NIOCore"]),
    .library(name: "NIOTestUtils", targets: ["NIOTestUtils"]),
    .library(name: "NIOTransportServices", targets: ["NIOTransportServices"]),
], dependencies: [.package(path: "../swift-log")], targets: [
    .target(name: "NIOCore", dependencies: [.product(name: "Logging", package: "swift-log")]),
    .target(name: "NIO", dependencies: ["NIOCore"]),
    .target(name: "NIOTestUtils", dependencies: ["NIO", "CValidation"]),
    .systemLibrary(name: "CValidation"),
    .target(name: "NIOTransportServices"),
])
