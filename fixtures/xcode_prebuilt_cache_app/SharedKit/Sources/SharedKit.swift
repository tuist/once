// Declares package-visible API. Consumers inside the same package need the
// matching `-package-name` compiler flag, lowered from SWIFT_PACKAGE_NAME.
package struct SharedModel {
    package var value = 7

    package init() {}
}

public func sharedDescription() -> String {
    "shared"
}
