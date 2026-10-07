import Core
import SwiftUI

@main
struct HelloApp: App {
    var body: some Scene {
        WindowGroup {
            Text(greeting())
                .padding()
        }
    }
}
