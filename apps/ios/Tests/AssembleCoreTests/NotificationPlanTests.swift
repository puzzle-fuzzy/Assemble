import Foundation
import Testing
@testable import AssembleCore

struct NotificationPlanTests {
    static let now: Int64 = 1_794_400_000_000

    static func content(
        _ title: String, startAt: Int64, endAt: Int64? = nil, repeating: Repeat = .none,
        repeatUntil: Int64? = nil, remindBefore: UInt32? = nil, notes: String? = nil,
        status: ScheduleStatus = .upcoming
    ) -> Schedule {
        Schedule(
            id: "s-\(title)", version: 1, status: status, createdBy: "测试",
            createdAt: WireTime(millis: now - 1_000_000), updatedAt: WireTime(millis: now - 1_000_000),
            title: title, location: nil, startAt: WireTime(millis: startAt), endAt: endAt.map { WireTime(millis: $0) },
            repeatRule: repeating, repeatUntil: repeatUntil.map { WireTime(millis: $0) },
            remindBefore: remindBefore, notes: notes
        )
    }

    static func reminder(_ id: String, title: String = "提醒", dueAt: Int64, status: ReminderStatus = .scheduled, notes: String? = nil) -> Reminder {
        Reminder(
            id: id, version: 1, status: status, createdBy: "测试",
            createdAt: WireTime(millis: now - 1_000_000), updatedAt: WireTime(millis: now - 1_000_000),
            title: title, dueAt: WireTime(millis: dueAt), notes: notes
        )
    }

    static func planner(schedules: [Schedule] = [], reminders: [Reminder] = [], notifications: [PlannerNotification] = []) -> Planner {
        Planner(schedules: schedules, reminders: reminders, notifications: notifications)
    }

    let minute: Int64 = 60_000

    @Test
    func remindersThatAreStillAheadRingAtTheirDueMoment() {
        let planned = NotificationPlan.upcoming(
            in: Self.planner(reminders: [
                Self.reminder("r-now", dueAt: Self.now),
                Self.reminder("r-soon", title: "取快递", dueAt: Self.now + 3 * minute, notes: "带取件码"),
                Self.reminder("r-past", dueAt: Self.now - minute),
                Self.reminder("r-far", dueAt: Self.now + NotificationPlan.window + 1),
                Self.reminder("r-done", dueAt: Self.now + 5 * minute, status: .dismissed),
                Self.reminder("r-cancelled", dueAt: Self.now + 5 * minute, status: .cancelled),
                Self.reminder("r-fired", dueAt: Self.now + 5 * minute, status: .fired),
            ]),
            now: Self.now
        )
        #expect(planned.map(\.id) == ["n-r-now", "n-r-soon"])
        #expect(planned[0].fireAt == Self.now)
        #expect(planned[1].title == "取快递")
        #expect(planned[1].body == "带取件码")
        #expect(planned[1].target == .reminder("r-soon"))
    }

    @Test
    func schedulesRingLeadMinutesBeforeEachOccurrenceThatHasNotEnded() {
        let start = Self.now + 10 * minute
        let train = Self.now + 120 * minute
        let planned = NotificationPlan.upcoming(
            in: Self.planner(schedules: [
                Self.content("火车", startAt: train, remindBefore: 90, notes: "带身份证"),
                Self.content("准点的", startAt: Self.now + 2 * minute),
                Self.content("每天的", startAt: start, repeating: .daily, repeatUntil: start + 2 * DAY_MS),
                Self.content("结束了的", startAt: Self.now - 30 * minute, endAt: Self.now - 5 * minute),
                Self.content("宽限期里的", startAt: Self.now - 5 * minute, endAt: Self.now + 55 * minute),
                Self.content("太晚了的", startAt: Self.now - 20 * minute, endAt: Self.now + 40 * minute),
                Self.content("完成了的", startAt: Self.now + 4 * minute, status: .done),
                Self.content("取消了的", startAt: Self.now + 4 * minute, status: .cancelled),
            ]),
            now: Self.now
        )
        #expect(planned.map(\.id) == [
            "s-宽限期里的-\(Self.now - 5 * minute)",
            "s-准点的-\(Self.now + 2 * minute)",
            "s-每天的-\(start)",
            "s-火车-\(train)",
            "s-每天的-\(start + DAY_MS)",
            "s-每天的-\(start + 2 * DAY_MS)",
        ])
        // 提前 90 分钟的那一场，在它开始前 90 分钟响；宽限期里的那一场现在就该响。
        #expect(planned.map(\.fireAt) == [
            Self.now - 5 * minute, Self.now + 2 * minute, start, train - 90 * minute,
            start + DAY_MS, start + 2 * DAY_MS,
        ])
        #expect(planned[3].body == "带身份证")
        #expect(planned[3].occurrenceAt == train)
        #expect(planned[3].target == .schedule("s-火车", occurrenceAt: train))
    }

    @Test
    func theIdentifiersAreTheOnesTheServerUses() {
        // 样本里服务端生成的通知，标识就是"日程标识-场次开始的毫秒"和"n-提醒标识"。
        let planner = Fixtures.planner
        let train = Fixtures.schedule("c4cf4220b94fddee4f2fe490124b3fa9")!
        // 从两条通知生成之前的片刻来排：火车提前 90 分钟生成，提醒到点生成。
        let now = min(planner.notifications[0].raisedAt.millis, planner.notifications[1].raisedAt.millis) - 1
        let planned = NotificationPlan.upcoming(in: planner, now: now)
        let byID = Dictionary(uniqueKeysWithValues: planned.map { ($0.id, $0) })
        #expect(byID[planner.notifications[0].id]?.fireAt == train.startAt.millis - 90 * minute)
        #expect(byID[planner.notifications[0].id]?.body == "带身份证\n杭州东站 浙江省杭州市上城区全福桥路2号")
        #expect(byID[planner.notifications[0].id]?.occurrenceAt == train.startAt.millis)
        #expect(byID[planner.notifications[1].id]?.fireAt == planner.notifications[1].raisedAt.millis)
        #expect(byID[planner.notifications[1].id]?.target == .reminder("r-soon"))
    }

    @Test
    func anIdentifierIsTracedBackToItsItemWithOrWithoutTheServersRecord() {
        let planner = Fixtures.planner
        // 服务端生成过记录的：看记录。
        #expect(NotificationPlan.target(for: "n-r-soon", in: planner) == .reminder("r-soon"))
        let train = Fixtures.schedule("c4cf4220b94fddee4f2fe490124b3fa9")!
        #expect(
            NotificationPlan.target(for: "\(train.id)-\(train.startAt.millis)", in: planner)
                == .schedule(train.id, occurrenceAt: train.startAt.millis)
        )
        // 本机提前排好的那一场：服务端还没有记录，按标识的构成找。
        let weekly = Fixtures.schedule("s-weekly")!
        let someOccurrence = weekly.startAt.millis + 7 * 86_400_000
        #expect(
            NotificationPlan.target(for: "s-weekly-\(someOccurrence)", in: planner)
                == .schedule("s-weekly", occurrenceAt: someOccurrence)
        )
        // 名单里已经没有这一条了的。
        #expect(NotificationPlan.target(for: "n-r-gone", in: planner) == nil)
        #expect(NotificationPlan.target(for: "s-gone-\(someOccurrence)", in: planner) == nil)
        #expect(NotificationPlan.target(for: "s-weekly-soon", in: planner) == nil)
    }

    @Test
    func onlyTheNearestSixtyAreKept() {
        var schedules: [Schedule] = []
        for index in 0..<100 {
            schedules.append(Self.content("第\(index)项", startAt: Self.now + Int64(index + 1) * minute))
        }
        let planned = NotificationPlan.upcoming(in: Self.planner(schedules: schedules), now: Self.now)
        #expect(planned.count == NotificationPlan.limit)
        #expect(planned.first?.id == "s-第0项-\(Self.now + minute)")
        #expect(planned.last?.fireAt == Self.now + Int64(NotificationPlan.limit) * minute)
    }

    @Test
    func reconcilingOnlyTouchesWhatChanged() {
        func planned(_ id: String, _ fireAt: Int64) -> PlannedNotification {
            PlannedNotification(id: id, fireAt: fireAt, title: "t", body: nil, scheduleID: nil, reminderID: nil, occurrenceAt: nil)
        }
        let (add, remove) = NotificationPlan.reconcile(
            existing: ["kept": 10, "moved": 20, "gone": 30],
            wanted: [planned("kept", 10), planned("moved", 25), planned("new", 5)]
        )
        #expect(add.map(\.id) == ["moved", "new"])
        #expect(remove == ["gone"])
        let untouched = NotificationPlan.reconcile(existing: ["a": 1], wanted: [planned("a", 1)])
        #expect(untouched.add.isEmpty && untouched.remove.isEmpty)
    }

    @Test
    func catchUpOnlyShowsRecentUnreadNotificationsOnce() {
        func notification(_ id: String, raisedAgo: Int64) -> PlannerNotification {
            PlannerNotification(
                id: id, kind: .reminder, title: "t", body: nil, scheduleId: nil, reminderId: "r",
                occurrenceAt: nil, raisedAt: WireTime(millis: Self.now - raisedAgo)
            )
        }
        let fresh = NotificationPlan.catchUp(
            Self.planner(notifications: [
                notification("new", raisedAgo: 1_000),
                notification("seen", raisedAgo: 2_000),
                notification("old", raisedAgo: NotificationPlan.catchUpWindow + 1),
                notification("edge", raisedAgo: NotificationPlan.catchUpWindow),
            ]),
            now: Self.now,
            delivered: ["seen"]
        )
        #expect(fresh.map(\.id) == ["new", "edge"])
    }
}
