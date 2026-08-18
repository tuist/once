import XCTest
import SignalCore

final class SignalAppTests: XCTestCase {
    func testCoreExposesVersion() {
        XCTAssertEqual(SignalCore.version, "1.0")
    }
}
