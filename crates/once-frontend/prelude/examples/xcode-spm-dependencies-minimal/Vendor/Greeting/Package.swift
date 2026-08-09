// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "Greeting",
    products: [
        .library(name: "Greeting", targets: ["Greeting"]),
    ],
    targets: [
        .target(name: "Greeting"),
    ]
)
