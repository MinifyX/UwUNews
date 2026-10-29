import Foundation
import UserNotifications
import UwUNewsCore

/// Local notifications for new articles. The engine decides what is worth
/// one (the source's or a collection's bell is on, the source was fetched
/// before); this only says it.
enum Notifier {
    /// More than this at once becomes one summary.
    static let singleLimit = 4

    static func requestPermission() async -> Bool {
        let center = UNUserNotificationCenter.current()
        let settings = await center.notificationSettings()
        if settings.authorizationStatus == .notDetermined {
            return (try? await center.requestAuthorization(options: [.alert, .sound, .badge])) ?? false
        }
        return settings.authorizationStatus == .authorized || settings.authorizationStatus == .provisional
    }

    static func announce(_ notices: [Notice]) async {
        guard !notices.isEmpty else { return }
        let center = UNUserNotificationCenter.current()
        guard await center.notificationSettings().authorizationStatus == .authorized else { return }
        if notices.count > singleLimit {
            let content = UNMutableNotificationContent()
            content.title = String(localized: "\(notices.count) neue Artikel")
            let names = Array(NSOrderedSet(array: notices.map(\.sourceTitle))) as? [String] ?? []
            content.body = names.prefix(4).joined(separator: " · ")
            content.sound = .default
            content.threadIdentifier = "summary"
            try? await center.add(UNNotificationRequest(identifier: "summary-\(UUID())", content: content, trigger: nil))
            return
        }
        for notice in notices {
            let content = UNMutableNotificationContent()
            content.title = notice.collectionName.map { "\(notice.sourceTitle) · \($0)" } ?? notice.sourceTitle
            content.body = notice.title
            content.sound = .default
            content.threadIdentifier = notice.sourceId
            content.userInfo = ["article": notice.key]
            try? await center.add(UNNotificationRequest(identifier: notice.key, content: content, trigger: nil))
        }
    }
}
