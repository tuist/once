// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Root",
    products: [
        .library(name: "Root", type: .static, targets: ["Root"]),
    ],
    dependencies: [
        .package(path: "Vendor/Greeting"),
    ],
    targets: [
        .target(
            name: "Root",
            dependencies: [
                .product(name: "Greeting", package: "Greeting"),
            ]
        ),
    ]
)
