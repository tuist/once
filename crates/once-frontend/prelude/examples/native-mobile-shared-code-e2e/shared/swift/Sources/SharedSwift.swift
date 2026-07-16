@_cdecl("once_swift_answer")
public func onceSwiftAnswer() -> Int32 {
    7
}

@_cdecl("Java_dev_once_shared_MainActivity_swiftAnswer")
public func mainActivitySwiftAnswer(
    _ environment: UnsafeMutableRawPointer?,
    _ instance: UnsafeMutableRawPointer?
) -> Int32 {
    7
}
