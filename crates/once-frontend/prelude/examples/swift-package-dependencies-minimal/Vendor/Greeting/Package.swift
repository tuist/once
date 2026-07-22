// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Greeting",
    products: [
        .library(name: "Greeting", type: .static, targets: ["Greeting"]),
    ],
    targets: [
        .target(name: "Greeting"),
    ]
)
