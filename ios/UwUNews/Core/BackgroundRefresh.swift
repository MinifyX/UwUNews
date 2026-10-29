import BackgroundTasks
import Foundation
import UwUNewsCore

/// Fetching while the app is closed. iOS decides when (usually every half
/// hour to few hours, more often for apps that are used); this asks for the
/// earliest every 20 minutes and does the same as pull to refresh.
enum BackgroundRefresh {
    static let identifier = "app.uwunews.ios.refresh"

    static func schedule() {
        let request = BGAppRefreshTaskRequest(identifier: identifier)
        request.earliestBeginDate = Date(timeIntervalSinceNow: 20 * 60)
        try? BGTaskScheduler.shared.submit(request)
    }

    /// For SwiftUI's `.backgroundTask(.appRefresh(…))`.
    static func run() async {
        schedule()
        guard case let .success(engine) = AppModel.shared else { return }
        let report = await withTaskCancellationHandler {
            await AppModel.offValue { engine.refresh(sourceId: nil) }
        } onCancel: {
            engine.cancel()
        }
        await Notifier.announce(report.notices)
        guard !Task.isCancelled else { return }
        // What was read on another device, and what was read here.
        _ = await AppModel.off { () throws -> Bool in
            guard case .off = try engine.syncStatus().backend else {
                _ = try engine.syncNow()
                return true
            }
            return false
        }
    }
}
