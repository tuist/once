import Foundation
import MacroSupport
import Networking
import CatalogInterface
import CatalogModel

public struct CatalogRouter: CatalogRouting {
    public static let generatedIdentifier: String = #mockIdentifier("catalog")

    public init() {}

    public func openItem(id: String) -> String {
        "\(NetworkingMock.name)/catalog/\(id)"
    }

    public func title(for item: Item) -> String {
        let format = NSLocalizedString("catalog.title", bundle: Bundle(for: BundleToken.self), comment: "")
        return String(format: format, item.id)
    }
}

private final class BundleToken {}
