import Foundation

try "let generatedValue = 42\n".write(
    toFile: CommandLine.arguments[1], atomically: true, encoding: .utf8
)
