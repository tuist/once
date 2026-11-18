import SwiftUI

@main
struct HelloApp: App {
    var body: some Scene {
        WindowGroup {
            Text(greeting)
                .padding()
        }
    }

    private var greeting: String {
        guard let url = Bundle.main.url(forResource: "greeting", withExtension: "txt"),
              let text = try? String(contentsOf: url, encoding: .utf8) else {
            return "Greeting resource is missing"
        }
        return text.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
