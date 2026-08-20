// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "OnceNativeSwiftPackage",
    products: [
        .library(name: "OnceNativeSwiftPackage", targets: ["OnceNativeSwiftPackage"]),
    ],
    targets: [
        .target(name: "OnceNativeSwiftPackage"),
        .testTarget(name: "OnceNativeSwiftPackageTests", dependencies: ["OnceNativeSwiftPackage"]),
    ]
)
