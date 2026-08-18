import Foundation

public enum DesignSystem {
    public static let version = "1.0"
}

public protocol DesignTokenProviding {
    var accentColor: String { get }
}

public struct DefaultDesignTokens: DesignTokenProviding {
    public init() {}
    public let accentColor: String = "#3366CC"
}
