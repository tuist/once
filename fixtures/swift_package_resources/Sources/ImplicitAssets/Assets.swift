import AppKit

public func checkImplicitAssets() {
    let red = NSColor(named: "packageRed", bundle: .module)!.usingColorSpace(.deviceRGB)!
    let blue = NSColor(named: "packageBlue", bundle: .module)!.usingColorSpace(.deviceRGB)!
    precondition(red.redComponent > 0.9 && red.blueComponent < 0.1)
    precondition(blue.blueComponent > 0.9 && blue.redComponent < 0.1)
    precondition(NSColor(named: "excludedColor", bundle: .module) == nil)
    precondition(Bundle.module.url(forResource: "Contents", withExtension: "json") == nil)
}
