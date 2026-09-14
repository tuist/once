(.swift_package.deps | sort) == (["SwiftPackage_VaporGraph_Vapor", "SwiftPackage_VaporGraph_XCTVapor", "SwiftPackage_VaporGraph_VaporTesting"] | sort)
and .swift_package.attrs._default_test_roots == ["SwiftPackage_VaporGraph_VaporTests"]
and (.SwiftPackage_VaporGraph_Vapor.deps | sort) == (["SwiftPackage_VaporGraph_CVaporBcrypt", "SwiftPackage_swift-nio_NIO", "SwiftPackage_swift-nio_NIOCore", "SwiftPackage_swift-log_Logging"] | sort)
and .SwiftPackage_VaporGraph_Vapor.srcs == ["Sources/Vapor/Vapor.swift"]
and (.SwiftPackage_VaporGraph_Vapor.attrs.swift_flags | index("GRAPH_FIXTURE") != null)
and ."SwiftPackage_swift-nio_NIOCore".deps == ["SwiftPackage_swift-log_Logging"]
and (.SwiftPackage_VaporGraph_XCTVapor.deps | index("SwiftPackage_VaporGraph_VaporTestUtils") != null)
and (.SwiftPackage_VaporGraph_VaporTesting.deps | index("SwiftPackage_VaporGraph_VaporTestUtils") != null)
and (.SwiftPackage_VaporGraph_VaporTests.deps | index("SwiftPackage_swift-nio_NIOTestUtils") != null)
and (.SwiftPackage_VaporGraph_VaporTests.attrs.resources | index("Tests/VaporTests/Resources/file with spaces.txt") != null)
and ."SwiftPackage_swift-log_LoggingTests".kind == "apple_test_bundle"
and ."SwiftPackage_swift-nio_CValidation".kind == "apple_system_module"
and (."SwiftPackage_swift-nio_NIOTestUtils_MacroHost".deps | index("SwiftPackage_swift-nio_CValidation") != null)
