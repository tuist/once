import Testing
@testable import OnceNativeSwiftPackage

@Test func greetingIsAvailable() {
    #expect(greeting() == "Hello from Once")
    #expect(#filePath.hasPrefix("/"))
}
