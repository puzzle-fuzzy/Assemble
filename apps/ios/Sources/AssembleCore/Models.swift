import Foundation

// 线上的类型，和 `crates/assemble-protocol` 一一对应。字段名就是线上的拼法；
// 样本（`Fixtures/protocol.json`）由 Rust 侧生成，测试必须能把它原样解码。

/// 一台设备运行的平台。这一版代码不认识的值归到 `other`，和服务端的兜底一致。
public enum Platform: String, Codable, Hashable, Sendable {
    case windows
    case macos
    case linux
    case ios
    case other

    public init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = Platform(rawValue: raw) ?? .other
    }
}

public struct Account: Codable, Hashable, Sendable {
    public let id: String
    public let username: String
    public let displayName: String
    public let email: String?
    public let avatarUrl: String?
    public let createdAt: WireTime
}

public struct Device: Codable, Hashable, Sendable {
    public let id: String
    public let name: String
    public let platform: Platform
    public let online: Bool
    public let lastSeenAt: WireTime?
    public let createdAt: WireTime
}

/// `GET /v1/me`。
public struct MeResponse: Codable, Hashable, Sendable {
    public let account: Account
    public let device: Device
}

/// 一条日程的重复规则。
public enum Repeat: String, Codable, Hashable, Sendable {
    case none
    case daily
    case weekly
    case monthly
}

public enum ScheduleStatus: String, Codable, Hashable, Sendable {
    case upcoming
    case done
    case cancelled
}

/// `fired` 只由服务端在到点时写入。
public enum ReminderStatus: String, Codable, Hashable, Sendable {
    case scheduled
    case fired
    case dismissed
    case cancelled
}

/// 一处地点：`label` 是给人看的地名，`address` 是拿去导航的地址。
public struct Location: Codable, Hashable, Sendable {
    public let label: String
    public let address: String
}

public struct Schedule: Codable, Hashable, Sendable, Identifiable {
    public let id: String
    /// 每改一次加一。修改时要带上看到的那个版本，对不上就被拒绝。
    public let version: Int64
    public let status: ScheduleStatus
    public let createdBy: String
    public let createdAt: WireTime
    public let updatedAt: WireTime
    public let title: String
    public let location: Location?
    public let startAt: WireTime
    /// 结束时间。重复日程的每一场都持续同样长。
    public let endAt: WireTime?
    /// 线上叫 `repeat`，在 Swift 里是关键字。
    public let repeatRule: Repeat
    /// 重复到什么时候为止（含）。
    public let repeatUntil: WireTime?
    /// 提前多少分钟提醒；没有就是到点才提醒。
    public let remindBefore: UInt32?
    public let notes: String?

    enum CodingKeys: String, CodingKey {
        case id, version, status, createdBy, createdAt, updatedAt
        case title, location, startAt, endAt, repeatUntil, remindBefore, notes
        case repeatRule = "repeat"
    }

    /// 提前多少分钟提醒；没有和零是一样的。
    public var leadMinutes: Int64 {
        Int64(remindBefore ?? 0)
    }
}

public struct Reminder: Codable, Hashable, Sendable, Identifiable {
    public let id: String
    public let version: Int64
    public let status: ReminderStatus
    public let createdBy: String
    public let createdAt: WireTime
    public let updatedAt: WireTime
    public let title: String
    public let dueAt: WireTime
    public let notes: String?
}

/// 通知来自一条日程还是一条提醒。
public enum NotificationKind: String, Codable, Hashable, Sendable {
    case schedule
    case reminder
}

/// 到点之后生成的一条通知。名字里带着 `Planner`，免得和系统通知混在一起。
public struct PlannerNotification: Codable, Hashable, Sendable, Identifiable {
    public let id: String
    public let kind: NotificationKind
    public let title: String
    /// 给人看的几句话：备注、地点，或者这一场几点开始。
    public let body: String?
    public let scheduleId: String?
    public let reminderId: String?
    /// 日程的哪一场；提醒没有这一项。
    public let occurrenceAt: WireTime?
    public let raisedAt: WireTime
}

/// `GET /v1/planner`：一个账户的日程、提醒与还没读的通知。
public struct Planner: Codable, Hashable, Sendable {
    public let schedules: [Schedule]
    public let reminders: [Reminder]
    /// 还没读的通知，新的在前。
    public let notifications: [PlannerNotification]

    public init(schedules: [Schedule], reminders: [Reminder], notifications: [PlannerNotification]) {
        self.schedules = schedules
        self.reminders = reminders
        self.notifications = notifications
    }
}

// ---- 扫码配对 ----

/// 手机认领一张票据时提交的内容。
public struct PairingClaim: Codable, Hashable, Sendable {
    public let ticket: String
    /// S256 的挑战：校验器的 SHA-256，base64url。
    public let codeChallenge: String
    /// 这台手机在账户里叫什么；确认页上给人看的就是它。
    public let deviceName: String

    public init(ticket: String, codeChallenge: String, deviceName: String) {
        self.ticket = ticket
        self.codeChallenge = codeChallenge
        self.deviceName = deviceName
    }
}

/// 认领之后拿到的：只剩核对这一步。
public struct PairingClaimed: Codable, Hashable, Sendable {
    /// 六位数字，前面可以有 0。
    public let confirmationCode: String
    /// 这次配对到什么时候为止；过了就得重新扫码。
    public let expiresAt: WireTime
}

/// 拿着校验器来问结果时提交的内容。
public struct PairingFinish: Codable, Hashable, Sendable {
    public let ticket: String
    public let codeVerifier: String

    public init(ticket: String, codeVerifier: String) {
        self.ticket = ticket
        self.codeVerifier = codeVerifier
    }
}

/// 配对在手机这一侧看到的结果。
public enum PairingState: String, Codable, Hashable, Sendable {
    /// 网页上的人还没有点"允许"：接着问。
    case pending
    /// 允许了：令牌、账户和设备都在回应里。
    case complete
    /// 网页上的人点了"拒绝"。
    case denied
}

/// 问结果的回应。`state` 是 `complete` 时后三项才有。
public struct PairingOutcome: Codable, Hashable, Sendable {
    public let state: PairingState
    public let account: Account?
    public let device: Device?
    public let token: String?

    /// 已经拿到令牌时，交出登录要用的全部内容。
    public var signedIn: SignedInSession? {
        guard state == .complete, let account, let device, let token else { return nil }
        return SignedInSession(token: token, account: account, device: device)
    }
}

/// 出错时的正文：`{"error":{"code":"…","message":"…"}}`。靠 `code` 分辨该怎么办。
public struct ServerErrorBody: Codable, Hashable, Sendable {
    public struct Inner: Codable, Hashable, Sendable {
        public let code: String
        public let message: String?
    }

    public let error: Inner
}
