import AppKit

extension NSColor {
    static let brandPrimary = NSColor(named: "brandPrimary", bundle: .main)!
}

precondition(NSColor.brandPrimary.usingColorSpace(.deviceRGB)!.redComponent > 0.9)
print("asset color loaded")
