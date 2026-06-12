import SwiftUI
import Models

public struct ContentView: View {
    public init() {}

    public var body: some View {
        VStack(spacing: 16) {
            Text(Greeting.demo.title)
                .font(.largeTitle)
                .bold()
            Text(Greeting.demo.subtitle)
                .font(.body)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .padding(.horizontal)
        }
        .padding()
    }
}
