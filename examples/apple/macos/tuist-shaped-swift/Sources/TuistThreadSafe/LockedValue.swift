public struct LockedValue<Value> {
    public let value: Value

    public init(_ value: Value) {
        self.value = value
    }
}
