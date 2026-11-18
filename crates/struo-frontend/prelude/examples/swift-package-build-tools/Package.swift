// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "BuildTools",
    products: [.library(name: "Generated", targets: ["Generated"])],
    targets: [
        .target(name: "Generated", plugins: [.plugin(name: "Generate")]),
        .plugin(name: "Generate", capability: .buildTool(), dependencies: ["Generator"]),
        .executableTarget(name: "Generator"),
    ]
)
