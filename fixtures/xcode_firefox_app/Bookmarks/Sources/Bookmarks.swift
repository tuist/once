import BrowserKit

public struct Bookmark {
    public let url: String
    public let title: String
    public init(url: String, title: String) {
        self.url = url
        self.title = title
    }
}

public enum Bookmarks {
    public static func defaultList() -> [Bookmark] {
        return [
            Bookmark(url: "https://mozilla.org", title: "Mozilla"),
            Bookmark(url: "https://firefox.com", title: "Firefox"),
        ]
    }
}
