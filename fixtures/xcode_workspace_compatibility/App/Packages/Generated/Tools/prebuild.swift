import Foundation

let output = URL(fileURLWithPath: CommandLine.arguments[1])
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
if CommandLine.arguments.count == 2 {
    try "let prebuildValue = 2\n".write(to: output.appending(path: "Prebuild.swift"), atomically: true, encoding: .utf8)
    let fixtures = output.appending(path: "Fixtures")
    try FileManager.default.createDirectory(at: fixtures, withIntermediateDirectories: true)
    try "<RESOURCE_TEMPLATE>".write(to: fixtures.appending(path: "template.swift"), atomically: true, encoding: .utf8)
}
