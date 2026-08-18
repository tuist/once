import Foundation

public struct SearchResult {
    public let query: String
    public init(query: String) { self.query = query }
}

public enum Search {
    public static func perform(_ query: String) -> SearchResult {
        return SearchResult(query: query)
    }
}
