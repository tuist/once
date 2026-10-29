import Foundation
import GeneratorSupport

let arguments = CommandLine.arguments
let environment = ProcessInfo.processInfo.environment
guard environment["GENERATOR_CONTEXT"] == "declared:environment" else {
    fatalError("Missing declared plugin environment")
}
if arguments[1] == "header" {
    let output = URL(fileURLWithPath: arguments[2])
    try FileManager.default.createDirectory(at: output.deletingLastPathComponent(), withIntermediateDirectories: true)
    try "#define HEADER_VALUE \(environment["ONCE_HEADER_VALUE"] ?? "7")\n".write(to: output, atomically: true, encoding: .utf8)
    var source = "#include \"Config.h\"\n"
    if arguments[3].hasSuffix(".cpp") {
        source += "extern \"C\" int generatedHeaderValue(void) { int *value = new int(HEADER_VALUE); int result = *value; delete value; return result; }\n"
    } else {
        source += "int generatedHeaderValue(void) { return HEADER_VALUE; }\n"
    }
    if environment["ONCE_GENERATED_WARNING"] == "1" { source += "#warning Generated warning must be fatal\n" }
    try source.write(toFile: arguments[3], atomically: true, encoding: .utf8)
} else if arguments[1] == "lint" {
    guard environment["ONCE_PLUGIN_FAIL"] != "1" else { fatalError("Requested lint failure") }
    guard try String(contentsOfFile: arguments[3], encoding: .utf8).contains("buildToolValue") else {
        fatalError("Generated source must be ready before validation")
    }
    try "validated\n".write(toFile: arguments[2], atomically: true, encoding: .utf8)
} else {
    #if Feature
    let traitAdjustment = 0
    #else
    let traitAdjustment = 100
    #endif
    try "let buildToolValue = \(fixtureGeneratorValue + traitAdjustment) + prebuildValue\n".write(toFile: arguments[1], atomically: true, encoding: .utf8)
    try "payload".write(toFile: arguments[2], atomically: true, encoding: .utf8)
}
