import SharedKotlin
import SwiftUI

@_silgen_name("struo_shared_answer")
func struoSharedAnswer() -> Int32

@main
struct SharedApp: App {
    var body: some Scene {
        WindowGroup {
            Text("\(SharedKotlin().greeting()) · Rust \(struoSharedAnswer())")
                .padding()
        }
    }
}
