import Generated

public func macroResourceValue() -> String { generatedResource() }

@main struct HostProbe {
    static func main() {
        precondition(generatedValue() == 42)
        precondition(generatedResource() == "payload")
    }
}
