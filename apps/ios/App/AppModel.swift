import Foundation
import Observation
import OSLog
import AssembleCore

private let log = Logger(subsystem: "com.puzzlefuzzy.assemble.ios", category: "model")

extension Notification.Name {
    /// 用户点开了一条系统通知。`userInfo["id"]` 是那一条的标识。
    static let assembleNotificationTapped = Notification.Name("assemble.notificationTapped")
}

/// 应用的全部状态与动作。界面只读它、调它的方法；网络、钥匙串和系统通知都经它指名的
/// 几个依赖进出，所以测试可以把这几样换成替身。
///
/// 几条守住的规矩：令牌只在 `session` 和钥匙串里，不进任何给人看的文字；每一次异步
/// 回来都先核对是不是同一轮（`generation`）——退出登录、换账户之后，旧请求的结果一律丢掉；
/// 退出登录以服务端确认为准，服务端没确认之前不在本机忘掉。
@MainActor
@Observable
final class AppModel {
    enum Phase: Equatable {
        case loading
        case signedOut
        case signedIn
    }

    enum PairingProgress: Equatable {
        case idle
        case claiming
        case confirm(code: String, until: Int64)
        case failed(String)
    }

    // ---- 依赖 ----
    private let origin: URL
    private let api: AssembleAPI
    private let store: SessionStoring
    private let scheduler: NotificationScheduling
    private let now: @Sendable () -> Int64
    private let deviceName: String
    private let defaults: UserDefaults?
    private let deliveredLimit = 2_000

    // ---- 状态 ----
    private(set) var phase = Phase.loading
    private(set) var session: SignedInSession?
    private(set) var planner: Planner?
    /// 给人看的一句话：上一步哪里不对。没有就是 `nil`。
    private(set) var problem: String?
    /// 正在做改动的那一个属于哪一代登录；同一时刻只做一个。
    private var busy: UUID?
    private(set) var remindersEnabled: Bool
    private(set) var notificationsAllowed = false
    /// 想打开到点提醒，系统却不让发通知。
    private(set) var remindersDenied = false
    private(set) var pairing = PairingProgress.idle
    /// 点开通知之后要定位到的那一条；界面定位完调用 `consumeFocus()`。
    private(set) var focus: NotificationTarget?
    /// 被点开的那一下才启动的应用，名单还没读出来：先记着标识，名单到了再定位。
    private var pendingOpen: String?
    /// 这一次登录里办完了的日程与提醒。提醒到点之后服务端要过十几秒才生成它的通知：用户抢在这之前点了
    /// "看过了"，那条通知晚一步才到名单里，来了也顺手标成已读，不然"到点的提醒"里会冒出一条已经办完的。
    private var handled: Set<String> = []

    /// 本机已经弹过的通知。名单每次都带着全部未读的通知，靠它知道哪几条已经弹过。
    private var delivered: Set<String>
    /// 上一次排进系统的每一条通知的样子：时刻变了或者文字变了都要重排。
    private var fingerprints: [String: String]
    private var generation = UUID()
    private var pairingTask: Task<Void, Never>?
    private var loop: Task<Void, Never>?
    /// 正在读名单的那一轮属于哪一代登录。同一代里重叠的请求合并成一次；换了一代就另起一轮。
    private var reading: UUID?
    private var refreshQueued = false

    init(
        origin: URL,
        api: AssembleAPI,
        store: SessionStoring,
        scheduler: NotificationScheduling,
        deviceName: String,
        now: @escaping @Sendable () -> Int64,
        defaults: UserDefaults?
    ) {
        self.origin = origin
        self.api = api
        self.store = store
        self.scheduler = scheduler
        self.deviceName = deviceName
        self.now = now
        self.defaults = defaults
        let key = origin.absoluteString
        remindersEnabled = defaults?.bool(forKey: "assemble.reminders.\(key)") ?? false
        delivered = Set(defaults?.stringArray(forKey: "assemble.delivered.\(key)") ?? [])
        fingerprints = defaults?.dictionary(forKey: "assemble.fingerprints.\(key)") as? [String: String] ?? [:]
    }

    /// 正式运行：服务地址在构建时定下，登录存系统钥匙串，提醒用系统通知。
    @MainActor
    static func live(environment: [String: String] = ProcessInfo.processInfo.environment) -> AppModel {
        let bundled = Bundle.main.object(forInfoDictionaryKey: "AssembleServerOrigin") as? String
        let spelling = environment["ASSEMBLE_SERVER_ORIGIN"] ?? bundled ?? "http://127.0.0.1:8787"
        let origin = URL(string: spelling) ?? URL(string: "http://127.0.0.1:8787")!
        return AppModel(
            origin: origin,
            api: AssembleAPI(origin: origin),
            store: KeychainSessionStore(origin: origin),
            scheduler: SystemNotificationScheduler(),
            deviceName: UIDeviceSpelling.current,
            now: { Int64(Date().timeIntervalSince1970 * 1000) },
            defaults: .standard
        )
    }

    /// 服务的主机名，给账户页显示。
    var serviceLabel: String {
        origin.host ?? origin.absoluteString
    }

    var device: Device? {
        session?.device
    }

    var account: Account? {
        session?.account
    }

    // ---- 启动 ----

    func restore() async {
        guard phase == .loading else { return }
        do {
            if let saved = try store.load() {
                log.info("a stored session was found; reading the list")
                session = saved
                phase = .signedIn
                notificationsAllowed = await scheduler.status() == .authorized
                await refresh()
            } else {
                log.info("no stored session; showing the sign-in page")
                phase = .signedOut
            }
        } catch {
            log.error("the stored session could not be read: \(String(describing: error), privacy: .public)")
            problem = Words.storedSessionUnreadable
            phase = .signedOut
        }
    }

    func didBecomeActive() async {
        // 应用是被点开的那一下才启动的：那时界面还没立起来，消息没人收，标识记在 `NotificationTap` 里。
        if let id = NotificationTap.take() {
            open(notificationID: id)
        }
        guard phase == .signedIn else { return }
        await refresh()
        startLoop()
    }

    func didEnterBackground() {
        loop?.cancel()
        loop = nil
    }

    /// 回到前台之后每三十秒读一次名单；离开前台就停。
    private func startLoop() {
        loop?.cancel()
        loop = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(30))
                guard let self, !Task.isCancelled else { return }
                await self.refresh()
            }
        }
    }

    // ---- 扫码配对 ----

    /// 用户扫到了、或者粘贴进来了一个地址。
    func beginPairing(with text: String) {
        guard let ticket = PairingAddress.ticket(from: text) else {
            pairing = .failed(Words.badPairingAddress)
            return
        }
        cancelPairing(quietly: true)
        let proof = PKCE.generate()
        pairing = .claiming
        problem = nil
        pairingTask = Task { [weak self] in
            await self?.runPairing(ticket: ticket, proof: proof)
        }
    }

    func cancelPairing(quietly: Bool = false) {
        pairingTask?.cancel()
        pairingTask = nil
        pairing = .idle
        if !quietly {
            problem = nil
        }
    }

    private func runPairing(ticket: String, proof: PKCE.Proof) async {
        do {
            let claimed = try await api.claim(
                PairingClaim(ticket: ticket, codeChallenge: proof.challenge, deviceName: deviceName)
            )
            pairing = .confirm(code: claimed.confirmationCode, until: claimed.expiresAt.millis)
            let deadline = claimed.expiresAt.millis + 5_000
            while now() < deadline {
                try await Task.sleep(for: .seconds(2))
                try Task.checkCancellation()
                let outcome = try await api.finish(PairingFinish(ticket: ticket, codeVerifier: proof.verifier))
                switch outcome.state {
                case .pending:
                    continue
                case .denied:
                    pairing = .failed(Words.pairingDenied)
                    return
                case .complete:
                    guard let session = outcome.signedIn else {
                        throw APIError.badResponse
                    }
                    pairing = .idle
                    await signIn(session)
                    return
                }
            }
            pairing = .failed(Words.pairingExpired)
        } catch is CancellationError {
            // 用户取消了：状态已经由 `cancelPairing` 收拾好。
        } catch APIError.cancelled {
        } catch {
            pairing = .failed(Words.describe(error))
        }
    }

    private func signIn(_ session: SignedInSession) async {
        log.info("signed in; reading the list")
        generation = UUID()
        self.session = session
        phase = .signedIn
        problem = nil
        planner = nil
        do {
            try store.save(session)
        } catch {
            problem = "登录没有保存下来：下次打开应用要重新扫码。"
        }
        notificationsAllowed = await scheduler.status() == .authorized
        await refresh()
        startLoop()
    }

    // ---- 名单 ----

    /// 读一次名单，并照着它整理系统里排着的通知。同一代登录里重叠的请求合并成一次：
    /// 正在读的时候又有谁要读，记下来，读完再补一轮。
    func refresh() async {
        guard phase == .signedIn, let token = session?.token else { return }
        let mine = generation
        if reading == mine {
            log.info("a refresh is already running; another will follow it")
            refreshQueued = true
            return
        }
        reading = mine
        // 不论这一轮怎么结束都要放回去，包括读到一半换了一代（读到"不认"之后自己把登录忘掉）。
        // 2026-11-27 在模拟器里撞到过：标记没放回去，之后每一次读名单都只排队、不执行，
        // 名单一直停在"正在读取"。
        defer {
            if reading == mine {
                reading = nil
            }
        }
        while true {
            refreshQueued = false
            await readPlanner(token: token, mine: mine)
            // 补的那一轮必须真的跑：要求它的那个动作（标已读、看过了）改过的东西，前一轮的回应里未必有。
            // 中途换了一代的话，排队的那一次是别人的，留给他们。
            guard refreshQueued, mine == generation, reading == mine else { return }
        }
    }

    private func readPlanner(token: String, mine: UUID) async {
        do {
            let planner = try await api.signedIn(as: token).planner()
            // 令牌不进日志；只记拿到了多少条。
            let current = mine == generation
            log.info("planner fetched: \(planner.schedules.count) schedules, \(planner.reminders.count) reminders, \(planner.notifications.count) unread notifications, current=\(current)")
            guard mine == generation else { return }
            self.planner = planner
            problem = nil
            // 被点开的那一条在等名单：现在有了。名单里已经没有它的话，也就不等了。
            if let id = pendingOpen {
                pendingOpen = nil
                locate(id)
            }
            await settleNotifications(ofHandled: planner.notifications)
            await syncNotifications(for: planner)
        } catch APIError.cancelled {
            log.info("planner fetch cancelled")
        } catch {
            log.error("planner fetch failed: \((error as? APIError).map(String.init(describing:)) ?? "other", privacy: .public)")
            guard mine == generation else { return }
            if (error as? APIError)?.isAuthenticationFailure == true {
                forgetSession(explained: Words.sessionExpired)
                return
            }
            problem = Words.describe(error)
        }
    }

    private func syncNotifications(for planner: Planner) async {
        let instant = now()
        if !remindersEnabled {
            // 不弹提醒的手机也一样记下：以后打开提醒时，不该把积压的全弹一遍。
            delivered.formUnion(NotificationPlan.catchUp(planner, now: instant, delivered: delivered).map(\.id))
            persistLedger()
            return
        }
        guard notificationsAllowed else { return }

        let wanted = NotificationPlan.upcoming(in: planner, now: instant)
        let existing = await scheduler.pending()
        var remove: [String] = []
        var add: [PlannedNotification] = []
        // 交给了系统去响的那一些。它们记进账本：服务端到点之后生成的同一条通知来了，不再补弹一遍——
        // 不然同一条提醒会响两次。
        var handed: Set<String> = []
        for notification in wanted {
            let fingerprint = fingerprint(of: notification)
            if existing[notification.id] == notification.fireAt, fingerprints[notification.id] == fingerprint {
                // 系统里排着的正是它，不用动。
                handed.insert(notification.id)
            } else {
                remove.append(notification.id)
                add.append(notification)
            }
            fingerprints[notification.id] = fingerprint
        }
        for id in existing.keys where !wanted.contains(where: { $0.id == id }) {
            remove.append(id)
            fingerprints[id] = nil
        }
        if !remove.isEmpty {
            await scheduler.remove(remove)
        }
        for notification in add {
            do {
                try await scheduler.schedule(notification, body: body(for: notification))
                handed.insert(notification.id)
            } catch {
                // 没排进去就不记：等名单里出现它的记录时补弹，正好补上这一下。
            }
        }
        delivered.formUnion(handed)
        persistFingerprints()

        for notification in NotificationPlan.catchUp(planner, now: instant, delivered: delivered) {
            do {
                try await scheduler.present(id: notification.id, title: notification.title, body: body(for: notification))
                delivered.insert(notification.id)
            } catch {
                // 没弹成就不记账：下一次同步还会补。
            }
        }
        persistLedger()
    }

    private func fingerprint(of notification: PlannedNotification) -> String {
        "\(notification.fireAt)|\(notification.title)|\(notification.body ?? "")|\(notification.occurrenceAt.map(String.init) ?? "")"
    }

    /// 通知底下的几句话：日程先说几点开始，再是备注与地点。
    private func body(for notification: PlannedNotification) -> String {
        var lines: [String] = []
        if let occurrence = notification.occurrenceAt {
            lines.append("\(Words.clock(occurrence)) 开始")
        }
        if let body = notification.body {
            lines.append(body)
        }
        return lines.joined(separator: "\n")
    }

    private func body(for notification: PlannerNotification) -> String {
        var lines: [String] = []
        if let occurrence = notification.occurrenceAt {
            lines.append("\(Words.clock(occurrence.millis)) 开始")
        }
        if let body = notification.body {
            lines.append(body)
        }
        return lines.joined(separator: "\n")
    }

    // ---- 用户点开了一条通知 ----

    func open(notificationID id: String) {
        log.info("a notification was opened")
        // 同一下点开只处理一次：这里是唯一的入口，进来就把暂存的取走。
        NotificationTap.take()
        // 名单还没读出来（应用刚被点开）时先记着，读出来再定位。
        if !locate(id) {
            pendingOpen = id
        }
    }

    /// 把某一条通知对应的那一条摆到用户面前。名单里已经没有它了的，回 `false`。
    private func locate(_ id: String) -> Bool {
        guard let planner, let target = NotificationPlan.target(for: id, in: planner) else { return false }
        focus = target
        // 服务端生成过这条通知的，顺手标成已读；本机提前排好的那一些服务端还不认识，没什么可标的。
        if planner.notifications.contains(where: { $0.id == id }) {
            Task { await markRead([id]) }
        }
        return true
    }

    func consumeFocus() {
        focus = nil
    }

    // ---- 动作 ----

    func markRead(_ ids: [String]) async {
        await perform(handling: nil) { api in try await api.readNotifications(ids) }
    }

    func markAllRead() async {
        await perform(handling: nil) { api in try await api.readAllNotifications() }
    }

    /// "看过了"。提醒到点的那一瞬服务端自己会把它的版本号加一（待办 → 已到点），而用户点开通知就按这个，
    /// 手里多半还是加一之前的那一份：服务端回"被别处改过"。这不是别人改的——名单重新读过之后，它只是
    /// 变成了已到点，就照新的版本号再交一次。别的冲突（真的有人改过、已经办过了）照旧把说法留给用户。
    func dismiss(_ reminder: Reminder) async {
        let conflicted = await perform(handling: reminder.id) { api in
            _ = try await api.dismissReminder(reminder.id, version: reminder.version)
        }
        if conflicted, let fired = justFired(reminder) {
            await perform(handling: reminder.id) { api in
                _ = try await api.dismissReminder(fired.id, version: fired.version)
            }
        }
    }

    func cancel(_ reminder: Reminder) async {
        let conflicted = await perform(handling: reminder.id) { api in
            _ = try await api.cancelReminder(reminder.id, version: reminder.version)
        }
        if conflicted, let fired = justFired(reminder) {
            await perform(handling: reminder.id) { api in
                _ = try await api.cancelReminder(fired.id, version: fired.version)
            }
        }
    }

    /// 最新的名单里，这条提醒只是刚刚到了点（版本号变了，状态是已到点）；不是这样就是 `nil`。
    private func justFired(_ reminder: Reminder) -> Reminder? {
        planner?.reminders.first { $0.id == reminder.id && $0.status == .fired && $0.version != reminder.version }
    }

    func complete(_ schedule: Schedule) async {
        await perform(handling: schedule.id) { api in
            _ = try await api.completeSchedule(schedule.id, version: schedule.version)
        }
    }

    func cancel(_ schedule: Schedule) async {
        await perform(handling: schedule.id) { api in
            _ = try await api.cancelSchedule(schedule.id, version: schedule.version)
        }
    }

    /// 办完的日程与提醒，它们到点的通知一并标成已读：名单的"到点的提醒"里不留一条已经办完的。
    /// 标成了就把本地名单里的那几条也当场去掉，不用等下一回读名单；没标成不算这一步失败，下一次读名单再来。
    private func settleNotifications(ofHandled raised: [PlannerNotification]) async {
        guard let token = session?.token else { return }
        let ids = Set(
            raised.filter { notification in
                handled.contains(notification.reminderId ?? "") || handled.contains(notification.scheduleId ?? "")
            }.map(\.id)
        )
        guard !ids.isEmpty else { return }
        let mine = generation
        do {
            try await api.signedIn(as: token).readNotifications(Array(ids.sorted()))
        } catch {
            log.info("the notification of a handled item could not be read: \((error as? APIError).map(String.init(describing:)) ?? "other", privacy: .public)")
            return
        }
        guard mine == generation, let planner else { return }
        log.info("read \(ids.count) notification(s) of handled items")
        self.planner = Planner(
            schedules: planner.schedules,
            reminders: planner.reminders,
            notifications: planner.notifications.filter { !ids.contains($0.id) }
        )
    }

    /// 做一个改动，然后重新读名单。失败时把说法留给界面，名单照样重读一遍：
    /// 版本对不上、状态不对这两种错，只有看到最新的名单才知道下一步怎么办。
    /// `handling` 是这一步办完的那条日程或提醒：它到点的通知跟着标成已读（见 `handled`）。
    /// 回的是这一步有没有因为"别处改过"被拒（版本号对不上）。
    @discardableResult
    private func perform(
        handling itemID: String?,
        _ work: @Sendable (AssembleAPI) async throws -> Void
    ) async -> Bool {
        guard let token = session?.token, busy == nil else { return false }
        let mine = generation
        busy = mine
        // 不论怎么结束都放回去，包括中途换了一代（登录失效之后自己把登录忘掉）。
        defer {
            if busy == mine {
                busy = nil
            }
        }
        var conflicted = false
        do {
            try await work(api.signedIn(as: token))
            if let itemID {
                handled.insert(itemID)
            }
            problem = nil
        } catch APIError.cancelled {
        } catch {
            guard mine == generation else { return false }
            if (error as? APIError)?.isAuthenticationFailure == true {
                forgetSession(explained: Words.sessionExpired)
                return false
            }
            conflicted = (error as? APIError) == .versionConflict
            problem = Words.describe(error)
        }
        guard mine == generation else { return false }
        await settleNotifications(ofHandled: planner?.notifications ?? [])
        await refresh()
        return conflicted
    }

    /// 退出登录。以服务端确认为准：服务端没说"撤销了"，本机就不忘掉。
    func signOut() async {
        guard let token = session?.token, busy == nil else { return }
        let mine = generation
        busy = mine
        defer {
            if busy == mine {
                busy = nil
            }
        }
        do {
            try await api.signedIn(as: token).logout()
        } catch APIError.cancelled {
            return
        } catch {
            // 令牌已经被撤销（比如在别的设备上移除了这台）：服务端已经不认它，可以忘掉。
            if (error as? APIError)?.isAuthenticationFailure != true {
                problem = Words.signOutUnreachable
                return
            }
        }
        forgetSession(explained: nil)
    }

    private func forgetSession(explained: String?) {
        if explained == nil {
            log.info("the session was forgotten after signing out")
        } else {
            log.info("the server rejected the session; forgetting it")
        }
        generation = UUID()
        pairingTask?.cancel()
        loop?.cancel()
        pairingTask = nil
        loop = nil
        session = nil
        planner = nil
        focus = nil
        pendingOpen = nil
        handled = []
        pairing = .idle
        problem = explained
        phase = .signedOut
        do {
            try store.clear()
        } catch {
            // 钥匙串里的那一条留着也无害：令牌已经作废。下次登录会把它覆盖掉。
        }
        delivered = []
        fingerprints = [:]
        persistLedger()
        persistFingerprints()
        Task { await scheduler.removeAll() }
    }

    // ---- 到点提醒的开关 ----

    func setReminders(enabled: Bool) async {
        if enabled {
            let before = await scheduler.status()
            if before == .notDetermined {
                notificationsAllowed = await scheduler.requestAuthorization()
            } else {
                notificationsAllowed = before == .authorized
            }
            remindersEnabled = notificationsAllowed
            // 系统不让发通知（用户在系统的框里点过"不允许"，或者在设置里关了）：开关弹回去，
            // 账户页上留着一句怎么说，直到下一次打开成功。这句话不能放在 `problem` 里：
            // 那一个每回读名单成功都会清掉，撑不过三十秒。
            remindersDenied = !notificationsAllowed
            let after = notificationsAllowed
            log.info("reminders turned on; authorized before=\(before.rawValue, privacy: .public) after=\(after)")
            if notificationsAllowed {
                problem = nil
                if let planner {
                    await syncNotifications(for: planner)
                }
            }
        } else {
            remindersEnabled = false
            log.info("reminders turned off")
            await scheduler.removeAll()
            fingerprints = [:]
            persistFingerprints()
        }
        defaults?.set(remindersEnabled, forKey: "assemble.reminders.\(origin.absoluteString)")
    }

    private func persistLedger() {
        // 账本有上限。超了就只留名单里还用得着的：还没读的通知，和系统里正排着的——
        // 后者到了点会响，账本里得有它，服务端随后生成的那一条才不会再响一遍。别的标识以后不会再被问到。
        if delivered.count > deliveredLimit {
            let alive = Set(planner?.notifications.map(\.id) ?? []).union(fingerprints.keys)
            delivered = delivered.intersection(alive)
        }
        defaults?.set(Array(delivered), forKey: ledgerKey)
    }

    private func persistFingerprints() {
        defaults?.set(fingerprints, forKey: fingerprintKey)
    }

    private var ledgerKey: String {
        "assemble.delivered.\(origin.absoluteString)"
    }

    private var fingerprintKey: String {
        "assemble.fingerprints.\(origin.absoluteString)"
    }
}
