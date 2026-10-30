import Generated
import XCTest

final class FetchTests: XCTestCase {
    func testGeneratedValue() {
        XCTAssertEqual(generatedValue(), 42)
        XCTAssertEqual(generatedResource(), "payload")
    }
}
