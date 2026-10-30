import PackagePlugin

@main struct Generate: BuildToolPlugin {
    func createBuildCommands(context: PluginContext, target: Target) throws -> [Command] {
        let output = context.pluginWorkDirectoryURL.appending(path: "Generated.swift")
        return [.buildCommand(
            displayName: "Generate Swift source",
            executable: try context.tool(named: "Generator").url,
            arguments: [output.path],
            outputFiles: [output]
        )]
    }
}
