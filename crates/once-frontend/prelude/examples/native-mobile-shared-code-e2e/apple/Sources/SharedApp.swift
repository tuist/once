import SharedKotlin
import SwiftUI

@_silgen_name("once_shared_answer")
func onceSharedAnswer() -> Int32

@main
struct SharedApp: App {
    var body: some Scene {
        WindowGroup {
            Text("\(SharedKotlin().greeting()) · Rust \(onceSharedAnswer())")
                .padding()
        }
    }
}
