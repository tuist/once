import Foundation

public enum BrowserKit {
    public static let name = "BrowserKit"
}

public protocol ThemeProviding {
    var accentColor: String { get }
}

public struct DefaultTheme: ThemeProviding {
    public init() {}
    public let accentColor: String = "photon-blue"
}
