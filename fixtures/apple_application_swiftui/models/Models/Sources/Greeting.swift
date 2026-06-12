public struct Greeting: Sendable {
    public let title: String
    public let subtitle: String

    public init(title: String, subtitle: String) {
        self.title = title
        self.subtitle = subtitle
    }

    public static let demo = Greeting(
        title: "Hello from Once",
        subtitle: "Built without Xcode projects."
    )
}
