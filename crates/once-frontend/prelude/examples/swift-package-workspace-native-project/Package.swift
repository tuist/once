// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "OnceNativeSwiftPackage",
    products: [
        .library(name: "OnceNativeSwiftPackage", targets: ["OnceNativeSwiftPackage"]),
        .library(name: "OnceNativeDependency", targets: ["OnceNativeDependency"]),
        .library(name: "OnceNativeConsumer", targets: ["OnceNativeConsumer"]),
    ],
    targets: [
        .target(name: "OnceNativeSwiftPackage"),
        .target(name: "OnceNativeDependency"),
        .target(name: "OnceNativeConsumer", dependencies: ["OnceNativeDependency"]),
        .testTarget(name: "OnceNativeSwiftPackageTests", dependencies: ["OnceNativeSwiftPackage"]),
    ]
)
