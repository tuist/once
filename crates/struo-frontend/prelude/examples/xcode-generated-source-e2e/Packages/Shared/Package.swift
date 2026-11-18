// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "FixtureShared",
    products: [.library(name: "Shared", targets: ["Shared"])],
    targets: [.target(name: "Shared")]
)
