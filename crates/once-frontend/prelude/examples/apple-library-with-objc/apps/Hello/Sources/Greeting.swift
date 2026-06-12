import Foundation

public enum Greeting {
    public static func formatted(for name: String) -> String {
        let raw = Bridge.formatGreeting(name)
        return raw ?? "Hello, \(name)!"
    }
}
