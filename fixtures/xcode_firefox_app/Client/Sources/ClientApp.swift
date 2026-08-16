import BrowserKit
import Tabs
import Bookmarks

@main
struct ClientApp {
    static func main() {
        let tabs = Tabs.moduleName
        let bookmarks = Bookmarks.defaultList()
        print("firefox-fixture \(tabs) bookmarks=\(bookmarks.count)")
    }
}
