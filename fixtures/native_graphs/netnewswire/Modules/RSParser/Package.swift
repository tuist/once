// swift-tools-version: 6.0
import PackageDescription
let package = Package(name: "RSParser", products: [.library(name: "RSParser", type: .dynamic, targets: ["RSParser"])], dependencies: [.package(path: "../RSCore")], targets: [
    .target(name: "RSParser", dependencies: [.product(name: "RSCore", package: "RSCore")]),
])
