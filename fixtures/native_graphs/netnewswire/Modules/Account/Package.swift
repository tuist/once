// swift-tools-version: 6.0
import PackageDescription
let package = Package(name: "Account", products: [.library(name: "Account", type: .dynamic, targets: ["Account"])], dependencies: [.package(path: "../RSCore"), .package(path: "../RSParser")], targets: [
    .target(name: "Account", dependencies: [.product(name: "RSCore", package: "RSCore"), .product(name: "RSParser", package: "RSParser")]),
])
