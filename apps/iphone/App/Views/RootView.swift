import AssembleCore
import SwiftUI

/// 根视图：没登录是登录页，登录了是名单。前台与后台的切换、点开系统通知，都从这里转给模型。
struct RootView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        Group {
            switch model.phase {
            case .loading:
                ProgressView {
                    Text(Words.loading)
                }
            case .signedOut:
                SignInView()
            case .signedIn:
                NavigationStack {
                    HomeView()
                }
            }
        }
        .task {
            await model.restore()
            await model.didBecomeActive()
        }
        .onChange(of: scenePhase) { _, phase in
            Task {
                switch phase {
                case .active:
                    await model.didBecomeActive()
                case .background:
                    model.didEnterBackground()
                default:
                    break
                }
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: .assembleNotificationTapped)) { note in
            if let id = note.userInfo?["id"] as? String {
                model.open(notificationID: id)
            }
        }
    }
}

