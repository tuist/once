public protocol CatalogRouting: Sendable {
    func openItem(id: String) -> String
}
