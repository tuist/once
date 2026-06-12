#if SIM_BUILD
public let buildKind = "simulator"
#elseif DEVICE_BUILD
public let buildKind = "device"
#else
#error("select() did not resolve - expected SIM_BUILD or DEVICE_BUILD to be defined")
#endif

public func hello() -> String { buildKind }
