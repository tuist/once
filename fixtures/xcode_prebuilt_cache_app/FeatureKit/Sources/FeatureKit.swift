import SharedKit
import VendorText
import VendorUI

// `SharedModel` is package-visible: this only compiles when both SharedKit and
// FeatureKit receive the same `-package-name` from SWIFT_PACKAGE_NAME.
package func featurePackageValue() -> Int {
    SharedModel().value
}

public func runFeature() -> Int {
    // `render()` pulls the vendor UI archive (and, through its serialized
    // imports, VendorCore); `textValue()` comes from an interface-only
    // XCFramework whose textual interface imports VendorCore.
    render() + textValue() + featurePackageValue()
}
