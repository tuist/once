// A compiler plugin that speaks the plugin protocol directly (length-prefixed
// JSON on stdin and stdout), standing in for a prebuilt macro implementation
// without building swift-syntax. Every freestanding expansion returns the
// first string literal argument with a "-mock" suffix.
import Foundation

func readExactly(_ count: Int) -> Data? {
    var data = Data()
    while data.count < count {
        let chunk = FileHandle.standardInput.readData(ofLength: count - data.count)
        if chunk.isEmpty { return nil }
        data.append(chunk)
    }
    return data
}

func send(_ object: [String: Any]) {
    let payload = try! JSONSerialization.data(withJSONObject: object)
    var size = UInt64(payload.count).littleEndian
    var frame = withUnsafeBytes(of: &size) { Data($0) }
    frame.append(payload)
    FileHandle.standardOutput.write(frame)
}

func literal(in message: [String: Any]) -> String {
    let source = ((message["expandFreestandingMacro"] as? [String: Any])?["syntax"] as? [String: Any])?["source"] as? String ?? ""
    let parts = source.split(separator: "\"", omittingEmptySubsequences: false)
    return parts.count > 1 ? String(parts[1]) : "value"
}

while let header = readExactly(8) {
    let size = header.withUnsafeBytes { $0.loadUnaligned(as: UInt64.self) }.littleEndian
    guard let body = readExactly(Int(size)),
          let message = try? JSONSerialization.jsonObject(with: body) as? [String: Any] else { break }
    if message["getCapability"] != nil {
        send(["getCapabilityResult": ["capability": ["protocolVersion": 7]]])
    } else if message["expandFreestandingMacro"] != nil {
        send(["expandMacroResult": ["expandedSource": "\"\(literal(in: message))-mock\"", "diagnostics": [Any]()]])
    } else if message["expandAttachedMacro"] != nil {
        send(["expandAttachedMacroResult": ["expandedSources": [String](), "diagnostics": [Any]()]])
    } else if message["loadPluginLibrary"] != nil {
        send(["loadPluginLibraryResult": ["loaded": false, "diagnostics": [Any]()]])
    } else {
        break
    }
}
