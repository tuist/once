import CGenerated
import Foundation

#if DEBUG && !DEBUG_CONFIGURATION
#error("Custom debug configuration lost its conditional package setting")
#elseif !DEBUG && !RELEASE_CONFIGURATION
#error("Custom release configuration lost its conditional package setting")
#endif

public func generatedValue() -> Int {
    #if TUIST_BUILD
    return buildToolValue + Int(headerValue()) - 6
    #else
    return buildToolValue + Int(headerValue()) - 7
    #endif
}

public func generatedResource() -> String {
    let template = try! String(contentsOf: Bundle.module.url(forResource: "template", withExtension: "swift", subdirectory: "Fixtures")!, encoding: .utf8)
    precondition(template == "<RESOURCE_TEMPLATE>")
    return try! String(contentsOf: Bundle.module.url(forResource: "value", withExtension: "txt")!, encoding: .utf8)
}
