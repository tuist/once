import CatalogModel
import XCTest
@testable import CatalogFeature

final class CatalogFeatureTests: XCTestCase {
    func testRoutesThroughTheBinaryDependency() {
        XCTAssertEqual(CatalogRouter().openItem(id: "42"), "Networking/catalog/42")
    }

    func testMacroExpandsFromTheBinaryPlugin() {
        XCTAssertEqual(CatalogRouter.generatedIdentifier, "catalog-mock")
    }
}
