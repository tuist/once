import AppKit

public func checkDeclaredAssets() throws {
    precondition(NSColor(named: "declaredColor", bundle: .module) != nil)
    let message = Bundle.module.url(forResource: "message", withExtension: "txt")!
    let contents = try String(contentsOf: message, encoding: .utf8)
    precondition(contents == "package resource\n")
    let copied = Bundle.module.url(forResource: "Copied/Raw.xcassets/Contents", withExtension: "json")!
    precondition(FileManager.default.fileExists(atPath: copied.path))
}
