import XCTest
import BrowserKit
import Tabs
import Bookmarks

final class ClientTests: XCTestCase {
    func testBrowserKitExposesModuleName() {
        XCTAssertEqual(BrowserKit.name, "BrowserKit")
    }

    func testTabsModuleNameInheritsBrowserKitName() {
        XCTAssertEqual(Tabs.moduleName, "BrowserKit")
    }

    func testBookmarksProvidesDefaultList() {
        XCTAssertEqual(Bookmarks.defaultList().count, 2)
    }
}
