import CatalogInterface

public struct Item: Equatable, Sendable {
    public let id: String
    public let quantity: Int

    public init(id: String, quantity: Int) {
        self.id = id
        self.quantity = quantity
    }
}
