// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "ResourceFixtures",
    platforms: [.macOS(.v13)],
    products: [.executable(name: "ResourceProbe", targets: ["ResourceProbe"])],
    targets: [
        .target(name: "ImplicitAssets", exclude: ["Excluded"]),
        .target(name: "DeclaredAssets", resources: [.process("Resources"), .copy("Copied")]),
        .executableTarget(name: "ResourceProbe", dependencies: ["ImplicitAssets", "DeclaredAssets"]),
    ]
)
