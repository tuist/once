import DesignSystem
import Article
import Search
import Settings

@main
struct WikipediaApp {
    static func main() {
        let designVersion = DesignSystem.version
        let articles = Articles.seed()
        let search = Search.perform("wikipedia")
        let language = Settings.defaultLanguage
        print("wikipedia-fixture \(designVersion) articles=\(articles.count) query=\(search.query) lang=\(language)")
    }
}
