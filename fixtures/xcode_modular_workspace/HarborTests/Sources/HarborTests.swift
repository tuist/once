import HarborComposition
import XCTest
@testable import Harbor

final class HarborTests: XCTestCase {
    func testHostApplicationLaunches() {
        XCTAssertNotNil(UIApplication.shared.delegate as? AppDelegate)
        XCTAssertEqual(Composition.identifier, "composition-mock")
    }
}
