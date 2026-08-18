import XCTest
import DesignSystem
import Article
import Search
import Settings

final class WikipediaTests: XCTestCase {
    func testDesignSystemExposesVersion() {
        XCTAssertEqual(DesignSystem.version, "1.0")
    }

    func testArticleSeedIsNonEmpty() {
        XCTAssertFalse(Articles.seed().isEmpty)
    }

    func testSearchReturnsResult() {
        let result = Search.perform("test")
        XCTAssertEqual(result.query, "test")
    }

    func testSettingsDefaults() {
        XCTAssertEqual(Settings.defaultLanguage, "en")
    }
}
