import TuistEnvironment
import TuistLogging
import TuistServer

public func runKit() -> String {
    "\(environmentName())|\(logPrefix())|\(serverSummary())"
}
