import AppCore

public struct Greeter {
    public init() {}

    public func greet(name: String) -> String {
        return Greeting().text(name: name)
    }
}
