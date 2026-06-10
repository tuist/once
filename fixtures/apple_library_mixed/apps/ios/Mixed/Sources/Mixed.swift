import Foundation

public struct Mixed {
    public init() {}

    public func describe() -> String {
        return "swift+\(MixedObjC.label())"
    }
}
