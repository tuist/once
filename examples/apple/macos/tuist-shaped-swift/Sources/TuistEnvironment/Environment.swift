import TuistConstants
import TuistThreadSafe

public func environmentName() -> String {
    LockedValue(Constants.productName).value
}
