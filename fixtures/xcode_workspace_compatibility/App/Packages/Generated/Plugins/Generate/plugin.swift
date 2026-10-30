import Foundation
import PackagePlugin

@main struct Generate: BuildToolPlugin {
    func createBuildCommands(context: PluginContext, target: Target) throws -> [Command] {
        let tool = try context.tool(named: "Generator").url
        let root = context.pluginWorkDirectoryURL
        let generated = root.appending(path: "Build.swift")
        let resource = root.appending(path: "value.txt")
        let prebuild = root.appending(path: "prebuild")
        let environment = ["GENERATOR_CONTEXT": "declared:environment"]
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        if target.name == "CGenerated" {
            let header = root.appending(path: "include/Config.h")
            let source = root.appending(path: ProcessInfo.processInfo.environment["ONCE_GENERATED_CXX"] == "1" ? "Generated.cpp" : "Generated.c")
            return [.buildCommand(displayName: "Generate C configuration", executable: tool,
                arguments: ["header", header.path, source.path], environment: environment, outputFiles: [header, source])]
        }
        try "this is not a declared generated source".write(to: root.appending(path: "Scratch.swift"), atomically: true, encoding: .utf8)
        let template = root.appending(path: "Template.swift")
        try "<GENERATED_VALUE>".write(to: template, atomically: true, encoding: .utf8)
        return [
            .prebuildCommand(displayName: "Discover generated Swift sources", executable: URL(fileURLWithPath: "/usr/bin/xcrun"),
                arguments: ["swift", context.package.directoryURL.appending(path: "Tools/prebuild.swift").path, prebuild.path],
                environment: environment, outputFilesDirectory: prebuild),
            .prebuildCommand(displayName: "Allow an empty prebuild result", executable: URL(fileURLWithPath: "/usr/bin/xcrun"),
                arguments: ["swift", context.package.directoryURL.appending(path: "Tools/prebuild.swift").path, root.appending(path: "empty").path, "empty"],
                environment: environment, outputFilesDirectory: root.appending(path: "empty")),
            .buildCommand(displayName: "Z generate Swift source", executable: tool,
                arguments: [generated.path, resource.path, template.path], environment: environment,
                inputFiles: [template], outputFiles: [generated, resource]),
            .buildCommand(displayName: "A validate package \(root.deletingLastPathComponent().lastPathComponent)", executable: tool,
                arguments: ["lint", root.appending(path: "lint.txt").path, generated.path], environment: environment,
                inputFiles: [generated]),
        ]
    }
}
