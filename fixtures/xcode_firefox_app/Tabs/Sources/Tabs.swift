import BrowserKit

public struct Tab {
    public let identifier: String
    public init(identifier: String) { self.identifier = identifier }
}

public enum Tabs {
    public static let moduleName = BrowserKit.name
}
