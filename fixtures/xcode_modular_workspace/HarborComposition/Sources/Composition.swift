import Analytics
import MacroSupport
import CatalogFeature
import CatalogInterface

public enum Composition {
    public static let identifier: String = #mockIdentifier("composition")

    public static func makeRouter() -> any CatalogRouting {
        _ = AnalyticsMock.name
        return CatalogRouter()
    }
}
