import Foundation
import Testing
@testable import AssembleCore

/// `Fixtures/protocol.json` 的形状。
struct ProtocolFixture: Decodable {
    struct Outcomes: Decodable {
        let pending: PairingOutcome
        let denied: PairingOutcome
        let complete: PairingOutcome
    }

    struct ErrorSample: Decodable {
        let status: Int
        let body: ServerErrorBody
    }

    let account: Account
    let devices: [Device]
    let me: MeResponse
    let pairingClaim: PairingClaim
    let pairingFinish: PairingFinish
    let pairingClaimed: PairingClaimed
    let pairingOutcomes: Outcomes
    let planner: Planner
    let errors: [ErrorSample]
}

enum Fixtures {
    static let wire: ProtocolFixture = {
        let data = try! Data(contentsOf: FixtureDirectory.url.appendingPathComponent("protocol.json"))
        return try! JSONDecoder().decode(ProtocolFixture.self, from: data)
    }()

    static var planner: Planner { wire.planner }

    static func schedule(_ id: String) -> Schedule? {
        planner.schedules.first { $0.id == id }
    }

    static func reminder(_ id: String) -> Reminder? {
        planner.reminders.first { $0.id == id }
    }
}

struct FixtureTests {
    @Test
    func everySampleDecodesIntoTheWireTypes() {
        let fixture = Fixtures.wire
        #expect(fixture.account.username == "烟熏三文鱼")
        #expect(fixture.account.avatarUrl?.hasPrefix("https://avatars.githubusercontent.com/") == true)
        #expect(fixture.devices.map(\.platform) == [.windows, .macos, .linux, .ios, .other])
        #expect(fixture.devices[0].lastSeenAt?.millis == 1_794_390_000_000)
        #expect(fixture.devices[3].lastSeenAt == nil)
        #expect(fixture.me.device.platform == .ios)

        #expect(fixture.pairingClaimed.confirmationCode == "004757")
        #expect(fixture.pairingOutcomes.pending.state == .pending)
        #expect(fixture.pairingOutcomes.pending.signedIn == nil)
        #expect(fixture.pairingOutcomes.denied.state == .denied)
        let signedIn = fixture.pairingOutcomes.complete.signedIn
        #expect(signedIn?.token == String(repeating: "t", count: 43))
        #expect(signedIn?.account == fixture.account)

        let planner = fixture.planner
        #expect(planner.schedules.count == 7)
        #expect(planner.reminders.count == 5)
        #expect(planner.notifications.count == 3)
        // 重复规则在 Swift 里叫 repeatRule，线上叫 repeat。
        #expect(planner.schedules.map(\.repeatRule) == [.none, .none, .daily, .weekly, .monthly, .none, .none])
        #expect(planner.schedules[0].remindBefore == 90)
        #expect(planner.schedules[1].remindBefore == nil)
        #expect(planner.schedules[1].title == "与 🐟 的会面：季度回顾")
        #expect(planner.schedules[0].location?.label == "杭州东站")
        #expect(planner.schedules[2].repeatUntil != nil)
        #expect(planner.schedules.map(\.status) == [.upcoming, .upcoming, .upcoming, .upcoming, .upcoming, .done, .cancelled])
        #expect(planner.reminders.map(\.status) == [.scheduled, .scheduled, .fired, .dismissed, .cancelled])
        #expect(planner.reminders[0].notes == "驿站 8 点关门\n带上取件码")
        #expect(planner.notifications.map(\.kind) == [.schedule, .reminder, .schedule])
        #expect(planner.notifications[2].body == nil)
        #expect(planner.notifications[1].reminderId == "r-soon")
        // 写回去：手机发给服务端的字段名也和样本一致。
        let claim = try! JSONEncoder().encode(fixture.pairingClaim)
        let claimJSON = try! JSONSerialization.jsonObject(with: claim) as! [String: Any]
        #expect(Set(claimJSON.keys) == ["ticket", "codeChallenge", "deviceName"])
        // 比内容不比字面：编码出来的字段先后每个进程都不一样，比字面会时红时绿（2026-11-27 撞到过）。
        let finish = try! JSONEncoder().encode(fixture.pairingFinish)
        let finishJSON = try! JSONSerialization.jsonObject(with: finish) as! [String: String]
        #expect(
            finishJSON == [
                "ticket": "abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE",
                "codeVerifier": String(repeating: "v", count: 43),
            ]
        )
    }

    @Test
    func aPlatformThisVersionDoesNotKnowReadsAsOther() throws {
        let json = #"{"id":"d","name":"将来","platform":"visionos","online":false,"createdAt":"2026-11-27T01:00:00.000Z"}"#
        let device = try JSONDecoder().decode(Device.self, from: Data(json.utf8))
        #expect(device.platform == .other)
        // 多出来的字段不影响：以后服务端加了字段，这一版照样读得出名单。
        let richer = #"{"schedules":[],"reminders":[],"notifications":[],"surprise":1}"#
        #expect(try JSONDecoder().decode(Planner.self, from: Data(richer.utf8)).schedules.isEmpty)
    }

    @Test
    func everyErrorSampleMapsToWhatThePhoneShouldDo() throws {
        for sample in Fixtures.wire.errors {
            let data = try JSONEncoder().encode(sample.body)
            let failure = APIFailure(status: sample.status, data: data)
            switch sample.body.error.code {
            case "INVALID_AUTH_CODE": #expect(failure == .invalidCode)
            case "TOO_MANY_ATTEMPTS": #expect(failure == .tooManyAttempts)
            case "UNAUTHORIZED": #expect(failure == .unauthorized)
            case "VERSION_CONFLICT": #expect(failure == .versionConflict)
            case "INVALID_STATE": #expect(failure == .invalidState)
            default: Issue.record("no expectation for \(sample.body.error.code)")
            }
        }
        // 正文不是那个形状时，只剩状态码可用。
        #expect(APIFailure(status: 404, data: Data("nope".utf8)) == .notFound)
        #expect(APIFailure(status: 503, data: Data("{}".utf8)) == .http(status: 503, code: nil))
    }
}

/// 测试里按服务端的答复合成的失败。
func APIFailure(status: Int, data: Data) -> APIError {
    AssembleAPI.failure(status: status, data: data)
}
