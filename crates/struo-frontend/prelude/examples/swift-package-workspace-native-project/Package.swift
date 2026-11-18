// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "StruoNativeSwiftPackage",
    products: [
        .library(name: "StruoNativeSwiftPackage", targets: ["StruoNativeSwiftPackage"]),
        .library(name: "StruoNativeDependency", targets: ["StruoNativeDependency"]),
        .library(name: "StruoNativeConsumer", targets: ["StruoNativeConsumer"]),
    ],
    targets: [
        .target(name: "StruoNativeSwiftPackage"),
        .target(name: "StruoNativeDependency"),
        .target(name: "StruoNativeConsumer", dependencies: ["StruoNativeDependency"]),
        .testTarget(name: "StruoNativeSwiftPackageTests", dependencies: ["StruoNativeSwiftPackage"]),
    ]
)
