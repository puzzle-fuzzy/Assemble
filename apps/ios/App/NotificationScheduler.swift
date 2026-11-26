import AssembleCore
import Foundation
import UserNotifications

/// 系统通知在本应用里的用法。正式运行是 `UNUserNotificationCenter`，测试给一个替身。
protocol NotificationScheduling: Sendable {
    func status() async -> UNAuthorizationStatus
    /// 第一次会问用户；已经拒绝过的不再问，直接回 `false`。
    func requestAuthorization() async -> Bool
    /// 已经排着的通知：标识到它该响的时刻。
    func pending() async -> [String: Int64]
    /// 排一条到点才响的。
    func schedule(_ notification: PlannedNotification, body: String) async throws
    /// 现在就响一条。
    func present(id: String, title: String, body: String) async throws
    func remove(_ ids: [String]) async
    func removeAll() async
}

struct SystemNotificationScheduler: NotificationScheduling {
    // 系统通知的中心不是可以跨线程交出去的东西，所以每次现取那个单例。
    private var center: UNUserNotificationCenter {
        UNUserNotificationCenter.current()
    }

    func status() async -> UNAuthorizationStatus {
        await center.notificationSettings().authorizationStatus
    }

    func requestAuthorization() async -> Bool {
        (try? await center.requestAuthorization(options: [.alert, .sound, .badge])) ?? false
    }

    func pending() async -> [String: Int64] {
        var result: [String: Int64] = [:]
        for request in await center.pendingNotificationRequests() {
            guard let trigger = request.trigger else { continue }
            let date: Date?
            if let calendar = trigger as? UNCalendarNotificationTrigger {
                date = calendar.nextTriggerDate()
            } else if let interval = trigger as? UNTimeIntervalNotificationTrigger {
                date = interval.nextTriggerDate()
            } else {
                date = nil
            }
            if let date {
                result[request.identifier] = Int64(date.timeIntervalSince1970 * 1000)
            }
        }
        return result
    }

    func schedule(_ notification: PlannedNotification, body: String) async throws {
        let content = UNMutableNotificationContent()
        content.title = notification.title
        content.body = body
        content.sound = .default
        // 点开通知时按标识找回那一条：标识就是服务端通知的标识。
        content.userInfo = ["assemble.id": notification.id]

        let wait = notification.date.timeIntervalSinceNow
        // 该响的时刻就在眼前（或者刚过去几分钟）：一秒之后响，交给系统去合并。
        let trigger: UNNotificationTrigger = wait > 1
            ? UNTimeIntervalNotificationTrigger(timeInterval: wait, repeats: false)
            : UNTimeIntervalNotificationTrigger(timeInterval: 1, repeats: false)
        try await center.add(UNNotificationRequest(identifier: notification.id, content: content, trigger: trigger))
    }

    func present(id: String, title: String, body: String) async throws {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.sound = .default
        content.userInfo = ["assemble.id": id]
        try await center.add(UNNotificationRequest(identifier: id, content: content, trigger: nil))
    }

    func remove(_ ids: [String]) async {
        center.removePendingNotificationRequests(withIdentifiers: ids)
    }

    func removeAll() async {
        center.removeAllPendingNotificationRequests()
    }
}
