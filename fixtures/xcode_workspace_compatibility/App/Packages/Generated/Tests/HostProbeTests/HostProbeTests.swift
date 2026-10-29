import HostProbe
import XCTest

final class HostProbeTests: XCTestCase {
    func testGeneratedResources() {
        XCTAssertEqual(macroResourceValue(), "payload")
    }
}
