import SwiftUI
import UserNotifications

@main
struct UwUNewsApp: App {
    @UIApplicationDelegateAdaptor private var delegate: AppDelegate
    @State private var model = AppModel()
    @Environment(\.scenePhase) private var phase

    init() {
        UwUFont.applyToBars()
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .tint(Palette.pinkSolid)
                .font(UwUFont.style(.body))
                .task {
                    delegate.model = model
                    await model.reload()
                    await model.refresh()
                }
        }
        .onChange(of: phase) { _, phase in
            switch phase {
            case .background:
                BackgroundRefresh.schedule()
            case .active:
                // Back after a while: fetch, unless that just happened.
                if let last = model.lastRefresh, last.timeIntervalSinceNow < -10 * 60 {
                    Task { await model.refresh() }
                }
            default:
                break
            }
        }
        .backgroundTask(.appRefresh(BackgroundRefresh.identifier)) {
            await BackgroundRefresh.run()
        }
    }
}

/// Opening a notification opens its article.
final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    @MainActor var model: AppModel?
    @MainActor static var pendingArticle: String?

    func application(
        _: UIApplication,
        didFinishLaunchingWithOptions _: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        return true
    }

    nonisolated func userNotificationCenter(
        _: UNUserNotificationCenter,
        willPresent _: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .list]
    }

    nonisolated func userNotificationCenter(
        _: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {
        let key = response.notification.request.content.userInfo["article"] as? String
        await MainActor.run {
            if let key {
                NotificationCenter.default.post(name: .openArticle, object: key)
                AppDelegate.pendingArticle = key
            }
        }
    }
}

extension Notification.Name {
    static let openArticle = Notification.Name("UwUNews.openArticle")
}
