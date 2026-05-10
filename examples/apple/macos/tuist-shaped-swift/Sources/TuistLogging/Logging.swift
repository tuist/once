import TuistConstants
import TuistEnvironment

public func logPrefix() -> String {
    "\(Constants.productName):\(environmentName())"
}
