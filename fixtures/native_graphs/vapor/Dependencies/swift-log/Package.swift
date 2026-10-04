// swift-tools-version: 6.0
import PackageDescription
let package = Package(name: "LoggingPackage", products: [.library(name: "Logging", targets: ["Logging"])], targets: [
    .target(name: "Logging"),
    .testTarget(name: "LoggingTests", dependencies: ["Logging"]),
])
