import SwiftUI
import UIKit
import UserNotifications

@main
struct AssembleApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model = AppModel.live()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
        }
    }
}

/// 这台手机叫什么：在账户的设备名单里就用它。
@MainActor
enum UIDeviceSpelling {
    static var current: String {
        UIDevice.current.name
    }
}

/// 用户点开的那一条系统通知的标识。应用可能正是被这一下点开的：那时候界面还没立起来，
/// 谁也收不到消息，所以先记在这里，等模型来取（`AppModel.open(notificationID:)`）。
@MainActor
enum NotificationTap {
    private(set) static var pending: String?

    /// 只记下来，不广播。
    static func stash(_ id: String) {
        pending = id
    }

    /// 系统报告用户点开了一条通知：先记下，再告诉已经在听的界面。
    static func record(_ id: String) {
        stash(id)
        NotificationCenter.default.post(name: .assembleNotificationTapped, object: nil, userInfo: ["id": id])
    }

    static func take() -> String? {
        defer { pending = nil }
        return pending
    }
}

final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    override init() {
        // 越早越好：应用是被点开的那一下才启动时，系统的报告在启动将完成时就来了，代理这时候就得在位。
        super.init()
        UNUserNotificationCenter.current().delegate = self
    }

    /// 应用在前台时也显示通知：用户正开着名单，到点的提醒照样要看得见。
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .sound, .list])
    }

    /// 用户点开了一条通知。
    ///
    /// 两个方法都用带回调的写法，不用 `async` 的那一种：那一种是在后台的协作线程上收尾的，UIKit 在收尾之后
    /// 要更新界面的快照，在非主线程上做这件事会断言崩溃。2026-11-27 在模拟器里点开通知时抓到过，
    /// 每一次点开必崩，崩溃报告 `Assemble-2026-11-27-002839.ips` 里就是这条调用栈。
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let id = response.notification.request.identifier
        Task { @MainActor in
            NotificationTap.record(id)
        }
        completionHandler()
    }
}
