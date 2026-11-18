import Testing
@testable import StruoNativeSwiftPackage

@Test func greetingIsAvailable() {
    #expect(greeting() == "Hello from Struo")
    #expect(#filePath.hasPrefix("/"))
}
