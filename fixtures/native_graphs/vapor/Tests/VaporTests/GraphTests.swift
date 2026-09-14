import XCTest
import XCTVapor
import VaporTesting
import Vapor
import NIOTestUtils
final class GraphTests: XCTestCase {
    func testGraph() { XCTAssertGreaterThan(graphValue(), 0) }
}
