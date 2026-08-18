import DesignSystem

public struct Article {
    public let title: String
    public init(title: String) { self.title = title }
}

public enum Articles {
    public static func seed() -> [Article] {
        return [
            Article(title: "Wikipedia Sample"),
        ]
    }
}
