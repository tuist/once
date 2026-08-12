import Foundation

public enum SignalCore {
    public static let version = "1.0"
}

public struct SignalProtocol {
    public let identifier: String
    public init(identifier: String) { self.identifier = identifier }
}
