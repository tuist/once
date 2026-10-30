// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "GeneratorSupport",
    products: [.library(name: "GeneratorSupport", targets: ["GeneratorSupport"])],
    targets: [.target(name: "GeneratorSupport")]
)
