import SignalCore

@main
struct SignalApp {
    static func main() {
        let core = SignalCore.version
        print("signal-fixture core=\(core)")
    }
}
