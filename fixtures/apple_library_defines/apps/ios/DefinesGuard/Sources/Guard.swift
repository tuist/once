#if !STRUO_DEFINES_PRESENT
#error("STRUO_DEFINES_PRESENT must be set via the `defines` attribute")
#endif

public struct Guard {
    public init() {}
    public func ok() -> Bool { return true }
}
