import AssembleCore

/// 详情页开着的时候，这一条在名单里变了：还留不留详情。纯规矩，放着好测。
enum DetailPolicy {
    /// 提醒到了点不算办完。到点的那一瞬服务端把它从"待办"改成"已到点"（版本号也加一），可用户点开通知
    /// 多半正是在这之后——详情要是跟着关上，他永远点不到"看过了"（2026-11-27 在模拟器里撞到过）。
    /// 办完了、不办了才收起来。
    static func staysOpen(reminder: Reminder) -> Bool {
        reminder.status == .scheduled || reminder.status == .fired
    }

    /// 日程完成或取消之后收起来。
    static func staysOpen(schedule: Schedule) -> Bool {
        schedule.status == .upcoming
    }
}
