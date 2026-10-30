// swift-tools-version: 6.3;(experimentalCGen)
import CompilerPluginSupport
import Foundation
import PackageDescription

let package = Package(
    name: "Generated",
    platforms: [.macOS(.v13)],
    products: [.library(name: "Generated", targets: ["Generated"])],
    traits: [.default(enabledTraits: ["Feature"]), .trait(name: "Feature")],
    dependencies: [.package(path: "../GeneratorSupport")],
    targets: [
        .target(name: "Generated", dependencies: ["CGenerated"], swiftSettings: [
            .define(ProcessInfo.processInfo.environment["FETCH_TUIST_BUILD"] == "1" ? "TUIST_BUILD" : "PLUGIN_BUILD"),
            .define("DEBUG_CONFIGURATION", .when(configuration: .debug)),
            .define("RELEASE_CONFIGURATION", .when(configuration: .release)),
        ], plugins: [.plugin(name: "Generate")]),
        .target(name: "CGenerated", cSettings: [.unsafeFlags(["-Wall"]), .treatAllWarnings(as: .error)], plugins: [.plugin(name: "Generate")]),
        .macro(name: "HostProbe", dependencies: ["Generated"]),
        .testTarget(name: "HostProbeTests", dependencies: ["HostProbe"]),
        .plugin(name: "Generate", capability: .buildTool(), dependencies: ["Generator"]),
        .executableTarget(name: "Generator", dependencies: [.product(name: "GeneratorSupport", package: "generatorsupport")]),
    ]
)
