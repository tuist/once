import HarborComposition
import UIKit

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    var window: UIWindow?

    func application(_ application: UIApplication, didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?) -> Bool {
        window = UIWindow(frame: UIScreen.main.bounds)
        let controller = UIViewController()
        controller.title = Composition.makeRouter().openItem(id: "launch")
        window?.rootViewController = controller
        window?.makeKeyAndVisible()
        return true
    }
}
