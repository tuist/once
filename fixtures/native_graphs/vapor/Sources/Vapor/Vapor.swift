import CVaporBcrypt
import NIO
import NIOCore
import Logging
#if GRAPH_FIXTURE
public func graphValue() -> Int { Int(graph_value()) + nioValue + coreValue + logValue }
#else
#error("Missing native target build setting")
#endif
