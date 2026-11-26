import Foundation

/// 手机上要提前排好的一条本地通知。标识用服务端通知的标识：提醒是 `n-<提醒标识>`，
/// 日程是 `<日程标识>-<场次开始的毫秒>`。这样本地排好的和以后从名单里读到的"没读的通知"
/// 是同一个标识，不会响两遍。
public struct PlannedNotification: Hashable, Sendable, Identifiable {
    public let id: String
    /// 该响的时刻。
    public let fireAt: Int64
    public let title: String
    public let body: String?
    public let scheduleID: String?
    public let reminderID: String?
    public let occurrenceAt: Int64?

    public var date: Date {
        Date(timeIntervalSince1970: TimeInterval(fireAt) / 1000)
    }

    /// 点开通知时要定位到的那一条。
    public var target: NotificationTarget? {
        if let scheduleID {
            return .schedule(scheduleID, occurrenceAt: occurrenceAt)
        }
        if let reminderID {
            return .reminder(reminderID)
        }
        return nil
    }
}

/// 一条通知对应名单里的哪一条。
public enum NotificationTarget: Hashable, Sendable {
    case schedule(String, occurrenceAt: Int64?)
    case reminder(String)
}

public enum NotificationPlan {
    /// 往后排这么多：系统至多留着 64 条待发的通知，排得太远没有意义。
    public static let window: Int64 = 30 * DAY_MS
    public static let limit = 60
    /// 提醒时刻已经过去这么久，或者这一场已经结束，就不再提醒——和服务端同一条规矩。
    public static let grace: Int64 = 10 * 60 * 1000
    /// 离线一阵之后，补弹最近这么久之内生成的通知——和桌面端同一条规矩。
    public static let catchUpWindow: Int64 = 24 * 60 * 60 * 1000

    /// 从现在起到 [`window`] 之内该响的，按时刻排好，取最近的 [`limit`] 条。
    ///
    /// 只排"还没到点的"：已经过了点的提醒由服务端生成的通知负责（见 [`catchUp`]）。
    /// 这一条规矩和服务端一致：离开提醒时刻超过 [`grace`] 的不提醒，这一场已经结束的不提醒。
    public static func upcoming(in planner: Planner, now: Int64) -> [PlannedNotification] {
        let horizon = now + window
        var planned: [PlannedNotification] = []

        for reminder in planner.reminders where reminder.status == .scheduled {
            let due = reminder.dueAt.millis
            if due < now || due > horizon {
                continue
            }
            planned.append(
                PlannedNotification(
                    id: "n-\(reminder.id)",
                    fireAt: due,
                    title: reminder.title,
                    body: reminder.notes,
                    scheduleID: nil,
                    reminderID: reminder.id,
                    occurrenceAt: nil
                )
            )
        }

        for schedule in planner.schedules where schedule.status == .upcoming {
            let lead = schedule.leadMinutes * 60_000
            let duration = (schedule.endAt?.millis ?? schedule.startAt.millis) - schedule.startAt.millis
            let rule = Recurrence(schedule: schedule)
            // 提醒时刻落在宽限期里的那一场也要：它现在就该响。
            for occurrence in rule.upcoming(from: now - lead - grace, limit: 400) {
                let fire = occurrence - lead
                if fire < now - grace || fire > horizon {
                    continue
                }
                if now >= occurrence + duration {
                    continue
                }
                var lines: [String] = []
                if let notes = schedule.notes {
                    lines.append(notes)
                }
                if let location = schedule.location {
                    lines.append("\(location.label) \(location.address)")
                }
                planned.append(
                    PlannedNotification(
                        id: "\(schedule.id)-\(occurrence)",
                        fireAt: fire,
                        title: schedule.title,
                        body: lines.isEmpty ? nil : lines.joined(separator: "\n"),
                        scheduleID: schedule.id,
                        reminderID: nil,
                        occurrenceAt: occurrence
                    )
                )
            }
        }

        // 同一个标识只留一条，按时刻排，取最近的若干条。
        planned.sort { lhs, rhs in
            if lhs.fireAt != rhs.fireAt {
                return lhs.fireAt < rhs.fireAt
            }
            return lhs.id < rhs.id
        }
        var seen: Set<String> = []
        var unique: [PlannedNotification] = []
        for notification in planned where seen.insert(notification.id).inserted {
            unique.append(notification)
        }
        return Array(unique.prefix(limit))
    }

    /// 一条通知的标识对应名单里的哪一条：用户点开通知时要定位到的目标。
    ///
    /// 服务端生成过的通知直接看它的记录。本机提前排好的那一些，服务端要到点之后十几秒才生成记录，
    /// 用户在这之间点开时没有记录可看，就按标识的构成找回名单里那一条：提醒是 `n-<提醒标识>`，
    /// 日程是 `<日程标识>-<场次的毫秒>`（从最后一个短横线切开，日程标识里带短横线也不怕）。
    /// 名单里已经没有这一条了的，回 `nil`。
    public static func target(for id: String, in planner: Planner) -> NotificationTarget? {
        if let notification = planner.notifications.first(where: { $0.id == id }) {
            if let scheduleID = notification.scheduleId {
                return .schedule(scheduleID, occurrenceAt: notification.occurrenceAt?.millis)
            }
            if let reminderID = notification.reminderId {
                return .reminder(reminderID)
            }
            return nil
        }
        if id.hasPrefix("n-") {
            let reminderID = String(id.dropFirst(2))
            return planner.reminders.contains { $0.id == reminderID } ? .reminder(reminderID) : nil
        }
        guard let dash = id.lastIndex(of: "-"),
              let occurrence = Int64(id[id.index(after: dash)...]),
              planner.schedules.contains(where: { $0.id == String(id[..<dash]) })
        else { return nil }
        return .schedule(String(id[..<dash]), occurrenceAt: occurrence)
    }

    /// 系统里已经排着的和现在该排的比一比：该加的、该撤的。没变的不动——
    /// 每次同步都全部重排的话，改过的提醒会被重复触发。
    public static func reconcile(
        existing: [String: Int64],
        wanted: [PlannedNotification]
    ) -> (add: [PlannedNotification], remove: [String]) {
        var add: [PlannedNotification] = []
        var keep: Set<String> = []
        for notification in wanted {
            keep.insert(notification.id)
            // 同一条通知的文字也可能改了；这里只看得出时刻，所以文字交给调用方一并更新。
            if existing[notification.id] != notification.fireAt {
                add.append(notification)
            }
        }
        let remove = existing.keys.filter { !keep.contains($0) }
        return (add, remove)
    }

    /// 离线一阵之后该补弹的：最近 [`catchUpWindow`] 之内生成的、本机还没弹过的没读通知。
    public static func catchUp(
        _ planner: Planner,
        now: Int64,
        delivered: Set<String>,
        window: Int64 = catchUpWindow
    ) -> [PlannerNotification] {
        planner.notifications.filter { notification in
            now - notification.raisedAt.millis <= window && !delivered.contains(notification.id)
        }
    }
}
