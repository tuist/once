import FeatureKit
import UIKit
import VendorData
import VendorEngine
import VendorRemote

final class AppDelegate: UIResponder, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
    ) -> Bool {
        // `engineNew()` loads the standalone engine archive member while the
        // UI archive already carries an older copy of the same engine: their
        // common symbols only coexist because DEAD_CODE_STRIPPING demotes the
        // duplicates. `dataValue()` resolves through SWIFT_INCLUDE_PATHS;
        // `remoteValue()` through an absolute out-of-workspace reference.
        let total = runFeature() + engineNew() + dataValue() + remoteValue()
        print("client total: \(total)")
        return true
    }
}

UIApplicationMain(CommandLine.argc, CommandLine.unsafeArgv, nil, NSStringFromClass(AppDelegate.self))
