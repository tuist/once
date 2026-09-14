// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "ExplicitModules",
    platforms: [.macOS(.v13)],
    products: [.executable(name: "NativeMain", targets: ["Main"])],
    targets: [
        .systemLibrary(name: "NativeValue", path: "Sources/NativeValue/include"),
        .target(name: "Left", dependencies: ["NativeValue"]),
        .target(name: "Right", dependencies: ["NativeValue"]),
        .executableTarget(name: "Main", dependencies: ["Left", "Right"]),
    ]
)
