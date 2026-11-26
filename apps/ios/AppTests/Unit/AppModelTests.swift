import AssembleCore
import Testing
import UserNotifications
@testable import Assemble

/// 不出门的"服务端"：按请求当场给出回应。
final class StubServer: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var answer: (@Sendable (String, Data?) -> Reply)?
    nonisolated(unsafe) static var paths: [String] = []

    struct Reply {
        var status = 200
        var body = Data()
        var headers: [String: String] = [:]

        static func json(_ status: Int, _ text: String) -> Reply {
            StubServer.Reply(status: status, body: Data(text.utf8), headers: ["Content-Type": "application/json"])
        }
    }

    static func reset() {
        answer = nil
        paths = []
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let body = request.httpBody ?? Self.read(request.httpBodyStream)
        let path = request.url?.path ?? ""
        Self.paths.append(path)
        guard let answer = Self.answer else {
            client?.urlProtocol(self, didFailWithError: URLError(.cannotConnectToHost))
            return
        }
        let reply = answer(path, body)
        let response = HTTPURLResponse(
            url: request.url!, statusCode: reply.status, httpVersion: "HTTP/1.1", headerFields: reply.headers
        )!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: reply.body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}

    private static func read(_ stream: InputStream?) -> Data? {
        guard let stream else { return nil }
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        while stream.hasBytesAvailable {
            let count = stream.read(&buffer, maxLength: buffer.count)
            guard count > 0 else { break }
            data.append(buffer, count: count)
        }
        return data
    }
}

/// 测试里能拨的钟。
final class Clock: @unchecked Sendable {
    var now: Int64 = 1_794_400_000_000
}

/// 记下每一次排布的"系统通知"。
final class FakeScheduler: NotificationScheduling, @unchecked Sendable {
    var authorization: UNAuthorizationStatus = .authorized
    var scheduled: [String: (fireAt: Int64, body: String)] = [:]
    var presented: [String] = []
    var removed: [String] = []
    var removeAllCalls = 0

    func status() async -> UNAuthorizationStatus { authorization }
    func requestAuthorization() async -> Bool {
        authorization = .authorized
        return true
    }

    func pending() async -> [String: Int64] {
        scheduled.mapValues(\.fireAt)
    }

    func schedule(_ notification: PlannedNotification, body: String) async throws {
        scheduled[notification.id] = (notification.fireAt, body)
    }

    func present(id: String, title: String, body: String) async throws {
        presented.append(id)
    }

    func remove(_ ids: [String]) async {
        removed += ids
        for id in ids {
            scheduled[id] = nil
        }
    }

    func removeAll() async {
        removeAllCalls += 1
        scheduled = [:]
    }
}

/// 样本由 Rust 侧生成，作为资源随测试包带上。
enum Sample {
    static var planner: Planner {
        let bundle = Bundle(for: StubServer.self)
        let url = bundle.url(forResource: "protocol", withExtension: "json", subdirectory: "Fixtures")!
        let data = try! Data(contentsOf: url)
        struct Wire: Decodable { let planner: Planner }
        return try! JSONDecoder().decode(Wire.self, from: data).planner
    }

    /// 样本里的名单，前面多一条刚刚生成的通知：离线期间到点的那条提醒。
    static var plannerWithAFreshNotification: Planner {
        let bundle = Bundle(for: StubServer.self)
        let url = bundle.url(forResource: "protocol", withExtension: "json", subdirectory: "Fixtures")!
        var root = try! JSONSerialization.jsonObject(with: try! Data(contentsOf: url)) as! [String: Any]
        var planner = root["planner"] as! [String: Any]
        var notifications = planner["notifications"] as! [[String: Any]]
        notifications.insert([
            "id": "n-late", "kind": "reminder", "title": "已经过了点还没人处理的", "reminderId": "r-overdue",
            "raisedAt": "2026-11-11T12:25:00.000Z",
        ], at: 0)
        planner["notifications"] = notifications
        root["planner"] = planner
        return try! JSONDecoder().decode(Planner.self, from: try! JSONSerialization.data(withJSONObject: root["planner"]!))
    }

    static func json<T: Encodable>(_ value: T) -> String {
        String(data: try! JSONEncoder().encode(value), encoding: .utf8)!
    }
}

@MainActor
@Suite(.serialized)
struct AppModelTests {
    let clock = Clock()
    let scheduler = FakeScheduler()
    let store = MemorySessionStore()
    let origin = URL(string: "https://assemble.example.test")!

    func model() -> AppModel {
        // 不动服务端现有的回答：好几条测试是先把回答备好，再要模型的。
        StubServer.paths = []
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [StubServer.self]
        return AppModel(
            origin: origin,
            api: AssembleAPI(origin: origin, session: AssembleAPI.makeSession(configuration: configuration)),
            store: store,
            scheduler: scheduler,
            deviceName: "测试的手机",
            now: { [clock] in clock.now },
            defaults: nil
        )
    }

    nonisolated static let sessionJSON = #"{"state":"complete","account":{"id":"u1","username":"alex","displayName":"Alex","email":null,"avatarUrl":null,"createdAt":"2026-11-11T12:26:40.123Z"},"device":{"id":"d1","name":"测试的手机","platform":"iphone","online":false,"createdAt":"2026-11-11T12:26:40.123Z"},"token":"\#(String(repeating: "t", count: 43))"}"#

    @Test
    func scanningTheCodeSignsThePhoneInOnceTheOwnerAllows() async throws {
        let model = model()
        await model.restore()
        #expect(model.phase == .signedOut)
        StubServer.answer = { path, _ in
            switch path {
            case "/v1/pairing/claim":
                return .json(200, #"{"confirmationCode":"123456","expiresAt":"2026-11-27T01:03:00.000Z"}"#)
            case "/v1/pairing/finish":
                return .json(200, Self.sessionJSON)
            case "/v1/planner":
                return .json(200, Sample.json(Sample.planner))
            default:
                return .json(404, #"{"error":{"code":"NOT_FOUND","message":"x"}}"#)
            }
        }

        model.beginPairing(with: "  assemble://pair?ticket=abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE\n")
        // 认领之后把六位数字给用户看，去电脑上核对。
        var waited = 0
        while model.pairing != .confirm(code: "123456", until: 1_795_741_380_000), waited < 50 {
            try await Task.sleep(for: .milliseconds(20))
            waited += 1
        }
        #expect(model.pairing == .confirm(code: "123456", until: 1_795_741_380_000))
        #expect(StubServer.paths.contains("/v1/pairing/claim"))
        #expect(model.phase == .signedOut, "本人点了允许之前不登录")

        // 等它自己问出结果。
        waited = 0
        while model.phase != .signedIn, waited < 400 {
            try await Task.sleep(for: .milliseconds(20))
            waited += 1
        }
        #expect(model.phase == .signedIn)
        #expect(model.pairing == .idle)
        #expect(model.account?.username == "alex")
        #expect(try store.load()?.token == String(repeating: "t", count: 43))
        #expect(model.planner?.schedules.count == 7)
    }

    @Test
    func aWrongAddressIsRefusedWithoutTouchingTheService() async throws {
        let model = model()
        await model.restore()
        model.beginPairing(with: "https://example.test/pair?ticket=abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE")
        #expect(model.pairing == .failed(Words.badPairingAddress))
        #expect(StubServer.paths.isEmpty)
        model.beginPairing(with: "assemble://pair?ticket=short")
        #expect(model.pairing == .failed(Words.badPairingAddress))
        #expect(StubServer.paths.isEmpty)
    }

    @Test
    func aDeniedPairingSaysSoAndSignsNobodyIn() async throws {
        let model = model()
        await model.restore()
        StubServer.answer = { path, _ in
            switch path {
            case "/v1/pairing/claim":
                return .json(200, #"{"confirmationCode":"000000","expiresAt":"2026-11-27T01:03:00.000Z"}"#)
            case "/v1/pairing/finish":
                return .json(200, #"{"state":"denied"}"#)
            default:
                return .json(404, "{}")
            }
        }
        model.beginPairing(with: "assemble://pair?ticket=abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE")
        var waited = 0
        while model.pairing == .claiming || model.pairing.isConfirming, waited < 400 {
            try await Task.sleep(for: .milliseconds(20))
            waited += 1
        }
        #expect(model.pairing == .failed(Words.pairingDenied))
        #expect(model.phase == .signedOut)
        #expect(try store.load() == nil)
    }

    @Test
    func aSavedSessionComesBackAndAnExpiredOneIsForgottenWithAnExplanation() async throws {
        let sample = try JSONDecoder().decode(PairingOutcome.self, from: Data(Self.sessionJSON.utf8))
        try store.save(sample.signedIn!)
        StubServer.answer = { path, _ in
            path == "/v1/planner" ? .json(200, Sample.json(Sample.planner)) : .json(404, "{}")
        }
        let model = model()
        await model.restore()
        #expect(model.phase == .signedIn)
        #expect(model.planner?.reminders.count == 5)

        // 服务端不再认这个令牌：回到登录页，并说清楚为什么。
        StubServer.answer = { _, _ in .json(401, #"{"error":{"code":"UNAUTHORIZED","message":"x"}}"#) }
        await model.refresh()
        #expect(model.phase == .signedOut)
        #expect(model.problem == Words.sessionExpired)
        #expect(try store.load() == nil, "服务端已经说了不认，本机才忘掉")
    }

    @Test
    func signingOutWaitsForTheServerToConfirm() async throws {
        let sample = try JSONDecoder().decode(PairingOutcome.self, from: Data(Self.sessionJSON.utf8))
        try store.save(sample.signedIn!)
        StubServer.answer = { path, _ in
            path == "/v1/planner" ? .json(200, Sample.json(Sample.planner)) : .json(404, "{}")
        }
        let model = model()
        await model.restore()
        #expect(model.phase == .signedIn)

        // 连不上服务：不退出，也不忘掉。
        StubServer.answer = nil
        await model.signOut()
        #expect(model.phase == .signedIn)
        #expect(model.problem == Words.signOutUnreachable)
        #expect(try store.load() != nil)

        // 服务端确认了：退出。
        StubServer.answer = { _, _ in StubServer.Reply(status: 204) }
        await model.signOut()
        #expect(model.phase == .signedOut)
        #expect(try store.load() == nil)
        #expect(StubServer.paths.contains("/v1/auth/logout"))
    }

    @Test
    func remindersAreLaidOutFromTheListAndOnlyWhatChangedIsTouched() async throws {
        let sample = try JSONDecoder().decode(PairingOutcome.self, from: Data(Self.sessionJSON.utf8))
        try store.save(sample.signedIn!)
        let model = model()
        // 提醒还关着的时候读名单：不排任何东西，没读的通知记进账本。
        StubServer.answer = { path, _ in
            path == "/v1/planner" ? StubServer.Reply.json(200, Sample.json(Sample.planner)) : StubServer.Reply.json(404, "{}")
        }
        await model.restore()
        #expect(scheduler.scheduled.isEmpty, "提醒的开关默认是关的")
        #expect(scheduler.presented.isEmpty)

        await model.setReminders(enabled: true)
        let laidOut = Set(scheduler.scheduled.keys)
        #expect(laidOut.contains("c4cf4220b94fddee4f2fe490124b3fa9-1795741200000"))
        #expect(laidOut.contains("s-daily-1795768800000"))
        #expect(laidOut.contains("n-r-soon"), "还没到点的提醒排在它的时刻")
        #expect(!laidOut.contains("n-r-overdue"), "已经过了点的提醒不排")
        // 打开提醒之前就没读的那一条不补弹：账本里有它，不然打开开关会把积压的全弹一遍。
        #expect(scheduler.presented.isEmpty)

        // 离线的时候服务端生成了一条新的通知：同步之后补弹一次，只弹一次。
        StubServer.answer = { path, _ in
            path == "/v1/planner" ? StubServer.Reply.json(200, Sample.json(Sample.plannerWithAFreshNotification)) : StubServer.Reply.json(404, "{}")
        }
        await model.refresh()
        #expect(scheduler.presented == ["n-late"])
        #expect(!Set(scheduler.scheduled.keys).contains("n-r-overdue"))

        // 再同步一次：什么都没变，就不该重排，也不再补弹。
        scheduler.removed = []
        await model.refresh()
        #expect(scheduler.removed.isEmpty)
        #expect(scheduler.presented == ["n-late"])

        // 关掉提醒：排着的全部撤掉。
        await model.setReminders(enabled: false)
        #expect(scheduler.scheduled.isEmpty)
    }

    @Test
    func anAnswerThatArrivesAfterSigningOutIsDropped() async throws {
        let sample = try JSONDecoder().decode(PairingOutcome.self, from: Data(Self.sessionJSON.utf8))
        try store.save(sample.signedIn!)
        final class Speed: @unchecked Sendable {
            var slow = false
        }
        let speed = Speed()
        StubServer.answer = { path, _ in
            if path == "/v1/planner" {
                if speed.slow {
                    Thread.sleep(forTimeInterval: 0.4)
                }
                return .json(200, Sample.json(Sample.planner))
            }
            return path == "/v1/auth/logout" ? StubServer.Reply(status: 204) : .json(404, "{}")
        }
        let model = model()
        await model.restore()
        #expect(model.phase == .signedIn)

        // 一次很慢的读取还在路上，用户退出了登录。
        model.didEnterBackground()
        speed.slow = true
        let late = Task { await model.refresh() }
        try await Task.sleep(for: .milliseconds(60))
        await model.signOut()
        #expect(model.phase == .signedOut)
        await late.value
        #expect(model.phase == .signedOut, "迟到的名单不能把人又变成登录状态")
        #expect(model.planner == nil)
    }

    /// 2026-11-27 在模拟器里撞到过：启动时读回的登录已经作废，服务端一说不认，"正在读"的标记就没放回去，
    /// 接着扫码登录成功，名单却一直停在"正在读取"。
    @Test
    func aSessionRejectedAtStartDoesNotStopTheNextSignInFromReadingTheList() async throws {
        let sample = try JSONDecoder().decode(PairingOutcome.self, from: Data(Self.sessionJSON.utf8))
        try store.save(sample.signedIn!)
        StubServer.answer = { path, _ in
            path == "/v1/planner" ? .json(401, #"{"error":{"code":"UNAUTHORIZED","message":"x"}}"#) : .json(404, "{}")
        }
        let model = model()
        await model.restore()
        #expect(model.phase == .signedOut)
        #expect(model.problem == Words.sessionExpired)

        StubServer.answer = { path, _ in
            switch path {
            case "/v1/pairing/claim":
                return .json(200, #"{"confirmationCode":"123456","expiresAt":"2026-11-27T01:03:00.000Z"}"#)
            case "/v1/pairing/finish":
                return .json(200, Self.sessionJSON)
            case "/v1/planner":
                return .json(200, Sample.json(Sample.planner))
            default:
                return .json(404, #"{"error":{"code":"NOT_FOUND","message":"x"}}"#)
            }
        }
        model.beginPairing(with: "assemble://pair?ticket=abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE")
        var waited = 0
        while model.phase != .signedIn || model.planner == nil, waited < 400 {
            try await Task.sleep(for: .milliseconds(20))
            waited += 1
        }
        #expect(model.phase == .signedIn)
        #expect(model.planner?.schedules.count == 7, "登录之后名单要读出来")
    }

    @Test
    func aRefreshAskedForWhileAnotherIsRunningStillHappens() async throws {
        let sample = try JSONDecoder().decode(PairingOutcome.self, from: Data(Self.sessionJSON.utf8))
        try store.save(sample.signedIn!)
        final class Speed: @unchecked Sendable {
            var slow = false
        }
        let speed = Speed()
        StubServer.answer = { path, _ in
            guard path == "/v1/planner" else { return .json(404, "{}") }
            if speed.slow {
                Thread.sleep(forTimeInterval: 0.4)
            }
            return .json(200, Sample.json(Sample.planner))
        }
        let model = model()
        await model.restore()
        #expect(model.planner?.notifications.contains { $0.id == "n-late" } == false)

        // 一轮很慢的读取还在路上，这时名单在服务端变了，又有人要求读一次：
        // 它要等前一轮读完之后真的再读一遍，而不是被前一轮"代表"掉。
        speed.slow = true
        let slow = Task { await model.refresh() }
        try await Task.sleep(for: .milliseconds(60))
        speed.slow = false
        StubServer.answer = { path, _ in
            path == "/v1/planner" ? .json(200, Sample.json(Sample.plannerWithAFreshNotification)) : .json(404, "{}")
        }
        await model.refresh()
        await slow.value
        #expect(model.planner?.notifications.contains { $0.id == "n-late" } == true)
    }

    /// 应用是被点开的那一下才启动的：名单还没读出来，定位不能丢。
    @Test
    func aNotificationTappedBeforeTheListIsReadIsLocatedOnceTheListArrives() async throws {
        let sample = try JSONDecoder().decode(PairingOutcome.self, from: Data(Self.sessionJSON.utf8))
        try store.save(sample.signedIn!)
        final class Speed: @unchecked Sendable {
            var slow = true
        }
        let speed = Speed()
        StubServer.answer = { path, _ in
            guard path == "/v1/planner" else { return .json(404, "{}") }
            if speed.slow {
                Thread.sleep(forTimeInterval: 0.4)
            }
            return .json(200, Sample.json(Sample.planner))
        }
        let model = model()
        let restoring = Task { await model.restore() }
        try await Task.sleep(for: .milliseconds(60))
        #expect(model.planner == nil, "名单还在路上")

        // 这时用户点开了一条通知：界面还没立起来，广播没人收，只有标识记在 `NotificationTap` 里。
        // （这里只记不广播：单元测试跑在应用自己的进程里，一广播，宿主应用真正的界面会把它取走。）
        // 它是本机提前排好的那一场：服务端还没有它的记录。
        speed.slow = false
        NotificationTap.stash("s-weekly-1795768800000")
        #expect(NotificationTap.pending == "s-weekly-1795768800000")
        await restoring.value
        #expect(model.focus == nil, "没人取，就还没定位")

        // 回到前台（启动的最后一步也是它）：取出来定位。
        await model.didBecomeActive()
        #expect(NotificationTap.pending == nil)
        #expect(model.focus == .schedule("s-weekly", occurrenceAt: 1_795_768_800_000))
        model.consumeFocus()

        // 名单早就在的时候点开一条提醒：当场定位，顺手把它标成已读。
        model.open(notificationID: "n-r-soon")
        #expect(model.focus == .reminder("r-soon"))
        var waited = 0
        while !StubServer.paths.contains("/v1/notifications/read"), waited < 100 {
            try await Task.sleep(for: .milliseconds(20))
            waited += 1
        }
        #expect(StubServer.paths.contains("/v1/notifications/read"))

        // 名单里已经没有这一条了的：不定位，也不留着。
        model.consumeFocus()
        model.open(notificationID: "n-r-gone")
        #expect(model.focus == nil)
    }

    /// 本机提前排好的那一条到点响过了，服务端随后才生成同一条通知：不能再响第二遍。
    /// 办完一条提醒，它到点的通知跟着标成已读。
    @Test
    func aNotificationThePhoneAlreadyRangIsNotRungAgainAndHandlingAReminderReadsItsNotification() async throws {
        let sample = try JSONDecoder().decode(PairingOutcome.self, from: Data(Self.sessionJSON.utf8))
        try store.save(sample.signedIn!)
        let model = model()
        // 提醒先开着，再读到名单：名单里既有还没到点的 r-soon，也有服务端为它生成过的通知 n-r-soon。
        await model.setReminders(enabled: true)
        StubServer.answer = { path, _ in
            path == "/v1/planner" ? StubServer.Reply.json(200, Sample.json(Sample.planner)) : StubServer.Reply.json(404, "{}")
        }
        await model.restore()
        #expect(scheduler.scheduled["n-r-soon"] != nil, "还没到点的那条提醒排进了系统")
        #expect(!scheduler.presented.contains("n-r-soon"), "系统会替它响，这里不再补弹一遍")

        // 从来没在这台手机上排过的一条（它到点的时候手机离线）：补弹一次，只弹它。
        scheduler.presented = []
        StubServer.answer = { path, _ in
            path == "/v1/planner" ? StubServer.Reply.json(200, Sample.json(Sample.plannerWithAFreshNotification)) : StubServer.Reply.json(404, "{}")
        }
        await model.refresh()
        #expect(scheduler.presented == ["n-late"])

        // 办完 r-soon：它那条通知 n-r-soon 跟着标成已读，别的通知不动。
        let soon = try #require(Sample.planner.reminders.first { $0.id == "r-soon" })
        let late = try #require(Sample.planner.notifications.first { $0.id == "n-r-soon" })
        #expect(late.reminderId == "r-soon")
        StubServer.answer = { path, _ in
            switch path {
            case "/v1/reminders/r-soon/dismiss":
                return .json(200, Sample.json(soon))
            case "/v1/notifications/read":
                return StubServer.Reply(status: 204)
            default:
                return .json(200, Sample.json(Sample.planner))
            }
        }
        StubServer.paths = []
        await model.dismiss(soon)
        #expect(StubServer.paths.contains("/v1/reminders/r-soon/dismiss"))
        #expect(StubServer.paths.contains("/v1/notifications/read"), "它那条通知跟着标成已读")
        #expect(model.planner?.notifications.contains { $0.id == "n-r-soon" } == false, "名单里当场去掉")
        #expect(model.problem == nil)
    }

    /// 用户手里的还是到点之前的那一份，这中间服务端到点了（版本号加一、状态变成已到点）：
    /// "看过了"被拒一次之后，照新的版本号再交一次，不能把这一下丢掉。
    @Test
    func dismissingAReminderTheServerJustFiredIsTriedAgainWithItsNewVersion() async throws {
        let sample = try JSONDecoder().decode(PairingOutcome.self, from: Data(Self.sessionJSON.utf8))
        try store.save(sample.signedIn!)
        let planner = Sample.planner
        let soon = try #require(planner.reminders.first { $0.id == "r-soon" })
        var row = try #require(try JSONSerialization.jsonObject(with: JSONEncoder().encode(soon)) as? [String: Any])
        row["status"] = "fired"
        row["version"] = 2
        let fired = try JSONDecoder().decode(Reminder.self, from: JSONSerialization.data(withJSONObject: row))
        let afterTheTick = Planner(
            schedules: planner.schedules,
            reminders: planner.reminders.map { $0.id == soon.id ? fired : $0 },
            notifications: planner.notifications
        )
        StubServer.answer = { path, body in
            switch path {
            case "/v1/planner":
                return .json(200, Sample.json(afterTheTick))
            case "/v1/reminders/r-soon/dismiss":
                let version = (try? JSONSerialization.jsonObject(with: body ?? Data()) as? [String: Any])?["version"] as? Int
                return version == 1 ? .json(409, #"{"error":{"code":"VERSION_CONFLICT","message":"x"}}"#) : .json(200, Sample.json(fired))
            case "/v1/notifications/read":
                return StubServer.Reply(status: 204)
            default:
                return .json(400, "{}")
            }
        }
        let model = model()
        await model.restore()
        StubServer.paths = []
        await model.dismiss(soon)
        let dismissals = StubServer.paths.filter { $0 == "/v1/reminders/r-soon/dismiss" }
        #expect(dismissals.count == 2, "第一下被拒，第二下按新的版本号交上去")
        #expect(model.problem == nil, "办成了就不留那句被别处改过")
    }

    /// 到点的那一瞬服务端把提醒标成"已到点"：详情不能跟着关上，用户点开的就是它到点的通知，
    /// 关上了就永远点不到"看过了"（2026-11-27 在模拟器里撞到过）。
    @Test
    func aReminderThatJustFiredKeepsItsDetailsOpen() throws {
        let reminders = Sample.planner.reminders
        #expect(DetailPolicy.staysOpen(reminder: try #require(reminders.first { $0.id == "r-soon" })))
        #expect(DetailPolicy.staysOpen(reminder: try #require(reminders.first { $0.id == "r-fired" })), "刚到了点不算办完")
        #expect(!DetailPolicy.staysOpen(reminder: try #require(reminders.first { $0.id == "r-dismissed" })))
        #expect(!DetailPolicy.staysOpen(reminder: try #require(reminders.first { $0.id == "r-cancelled" })))

        let schedules = Sample.planner.schedules
        #expect(DetailPolicy.staysOpen(schedule: schedules[0]))
        #expect(!DetailPolicy.staysOpen(schedule: try #require(schedules.first { $0.id == "s-done" })))
        #expect(!DetailPolicy.staysOpen(schedule: try #require(schedules.first { $0.id == "s-cancelled" })))
    }

    /// 用户抢在服务端生成通知之前就点了"看过了"：那条通知晚一步才到名单里，来了也要跟着算看过。
    @Test
    func aNotificationThatArrivesAfterItsReminderWasHandledIsReadToo() async throws {
        let sample = try JSONDecoder().decode(PairingOutcome.self, from: Data(Self.sessionJSON.utf8))
        try store.save(sample.signedIn!)
        let model = model()
        let soon = try #require(Sample.planner.reminders.first { $0.id == "r-soon" })
        // 到点之后的头十几秒：提醒还在，服务端还没为它生成通知。
        let quiet = Planner(schedules: Sample.planner.schedules, reminders: Sample.planner.reminders, notifications: [])
        StubServer.answer = { path, _ in
            switch path {
            case "/v1/planner":
                return .json(200, Sample.json(quiet))
            case "/v1/reminders/r-soon/dismiss":
                return .json(200, Sample.json(soon))
            default:
                return .json(400, "{}")
            }
        }
        await model.restore()
        await model.dismiss(soon)
        #expect(!StubServer.paths.contains("/v1/notifications/read"), "没什么可标的")

        // 下一回读名单，通知来了。
        StubServer.answer = { path, _ in
            switch path {
            case "/v1/planner":
                return .json(200, Sample.json(Sample.planner))
            case "/v1/notifications/read":
                return StubServer.Reply(status: 204)
            default:
                return .json(400, "{}")
            }
        }
        StubServer.paths = []
        await model.refresh()
        #expect(StubServer.paths.contains("/v1/notifications/read"), "办完的那一条，它迟到的通知跟着已读")
        #expect(model.planner?.notifications.contains { $0.id == "n-r-soon" } == false)
        #expect(model.planner?.notifications.contains { $0.id == "s-daily-1794391200000" } == true, "别的通知不动")
        #expect(model.problem == nil)
    }
}

extension AppModel.PairingProgress {
    var isConfirming: Bool {
        if case .confirm = self { true } else { false }
    }
}
