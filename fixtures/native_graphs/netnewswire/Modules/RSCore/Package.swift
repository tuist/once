// swift-tools-version: 6.0
import PackageDescription
let package = Package(name: "RSCore", products: [
    .library(name: "RSCore", type: .dynamic, targets: ["RSCore"]),
    .library(name: "RSCoreObjC", type: .dynamic, targets: ["RSCoreObjC"]),
], targets: [
    .target(name: "RSCore", dependencies: ["RSCoreObjC"]),
    .target(name: "RSCoreObjC", cSettings: [.headerSearchPath("include")]),
    .testTarget(name: "RSCoreTests", dependencies: ["RSCore"], resources: [.copy("Resources")]),
])
