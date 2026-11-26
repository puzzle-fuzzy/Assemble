/// 日程出现规则的推算，移植自 `crates/assemble-schedule/src/occurrences.rs`，逐行对照。
///
/// 手机上的本地通知要自己算"下一场是什么时候"，而到点时服务端也会按同一套规则生成
/// 它的通知。两边算得不一样，手机就会漏响或者响两遍，所以这里是照着 Rust 那一份写的，
/// 并由 `Fixtures/recurrence.json`（Rust 侧算出来的结果）钉住。
///
/// 时间一律用毫秒时间戳。月重复遵循固定的北京时间业务日历（UTC+08:00，不随夏令时变化）：
/// 先把时间戳整体平移八小时，在"平移后的日历"上做月份运算，再平移回来。
public let DAY_MS: Int64 = 86_400_000
public let WEEK_MS: Int64 = 7 * DAY_MS

/// 北京时间相对 UTC 的偏移。
public let SHANGHAI_OFFSET_MS: Int64 = 8 * 3_600_000

/// 月重复最多向前推算的步数：一千年。
public let MAX_STEPS: Int64 = 12_000

/// 一条日程的重复规则。
public struct Recurrence: Hashable, Sendable {
    /// 第一次出现的时刻。
    public let startAt: Int64
    public let rule: Repeat
    /// 重复截止时刻（含）。
    public let repeatUntil: Int64?

    public init(startAt: Int64, repeating rule: Repeat, repeatUntil: Int64?) {
        self.startAt = startAt
        self.rule = rule
        self.repeatUntil = repeatUntil
    }

    /// 从一条日程读出规则。
    public init(schedule: Schedule) {
        self.init(
            startAt: schedule.startAt.millis,
            repeating: schedule.repeatRule,
            repeatUntil: schedule.repeatUntil?.millis
        )
    }

    /// 不晚于 `now` 的最近一次出现。
    ///
    /// 日程还没开始、重复窗口已经关闭时返回 `nil`。只有最近这一次有意义：过去的出现不会被补发。
    public func latestAtOrBefore(_ now: Int64) -> Int64? {
        if now < startAt {
            return nil
        }
        let occurrence = Recurrence.atOrdinal(
            startAt, rule, Recurrence.ordinalAt(startAt, rule, now)
        )
        if let until = repeatUntil, occurrence > until {
            return nil
        }
        return occurrence
    }

    /// 从 `from` 起接下来的几次出现（含 `from` 当时刻），最多 `limit` 次。
    public func upcoming(from: Int64, limit: Int) -> [Int64] {
        if limit == 0 {
            return []
        }
        // `from` 前一刻所在的序号加一，就是第一个不早于它的出现。
        let first = max(Recurrence.ordinalAt(startAt, rule, Recurrence.momentBefore(from)) &+ 1, 0)
        var upcoming: [Int64] = []
        var ordinal = first
        while true {
            if rule == .none && ordinal > 0 {
                break
            }
            let occurrence = Recurrence.atOrdinal(startAt, rule, ordinal)
            if let until = repeatUntil, occurrence > until {
                break
            }
            upcoming.append(occurrence)
            if upcoming.count == limit {
                break
            }
            ordinal += 1
        }
        return upcoming
    }

    /// `at` 的前一刻；已经是最早的时刻时就原样交回。
    private static func momentBefore(_ at: Int64) -> Int64 {
        at == Int64.min ? Int64.max : at - 1
    }

    /// `at` 所在的出现序号；早于第一次出现时为 -1。
    static func ordinalAt(_ start: Int64, _ rule: Repeat, _ at: Int64) -> Int64 {
        if at < start {
            return -1
        }
        switch rule {
        case .daily:
            return (at - start) / DAY_MS
        case .weekly:
            return (at - start) / WEEK_MS
        case .monthly:
            return min(
                MAX_STEPS,
                max(0, Recurrence.monthsBetween(start + SHANGHAI_OFFSET_MS, at + SHANGHAI_OFFSET_MS))
            )
        case .none:
            return 0
        }
    }

    /// 第 `ordinal` 次出现的时刻。
    static func atOrdinal(_ start: Int64, _ rule: Repeat, _ ordinal: Int64) -> Int64 {
        switch rule {
        case .daily:
            return start &+ ordinal &* DAY_MS
        case .weekly:
            return start &+ ordinal &* WEEK_MS
        case .monthly:
            return Recurrence.monthOccurrence(start + SHANGHAI_OFFSET_MS, ordinal) - SHANGHAI_OFFSET_MS
        case .none:
            return start
        }
    }

    /// 从 `start` 到 `current` 为止，最后一个不晚于 `current` 的月度序号；假定 `start <= current`。
    private static func monthsBetween(_ start: Int64, _ current: Int64) -> Int64 {
        let from = Civil(millis: start)
        let to = Civil(millis: current)
        var months = (to.year - from.year) * 12 + Int64(to.month) - Int64(from.month)
        if monthOccurrence(start, months) > current {
            months -= 1
        }
        return months
    }

    /// `start` 之后第 `months` 个月的同一天；短月份钳制到当月最后一天。
    private static func monthOccurrence(_ start: Int64, _ months: Int64) -> Int64 {
        let date = Civil(millis: start)
        // 月份按"年 × 12 + 月"连续编号，跨年与负数都由除法自然处理。
        let total = date.year * 12 + Int64(date.month) + months
        let year = total.dividingFloor(12)
        let month = total.moduloFloor(12)
        let day = min(Int64(date.day), Int64(daysInMonth(year, month: Int(month))))
        return Civil(year: year, month: Int(month), day: Int(day)).millis
            + start.moduloFloor(DAY_MS)
    }

    private static func daysInMonth(_ year: Int64, month: Int) -> Int {
        switch month {
        case 0, 2, 4, 6, 7, 9, 11:
            return 31
        case 3, 5, 8, 10:
            return 30
        case 1:
            return isLeap(year) ? 29 : 28
        default:
            return 0
        }
    }

    private static func isLeap(_ year: Int64) -> Bool {
        year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
    }
}

/// 公历日期：`month` 从 0 开始，沿用 Rust 那一份的写法，方便逐行对照。
private struct Civil {
    let year: Int64
    let month: Int
    let day: Int

    /// 毫秒时间戳对应的日期。
    init(millis: Int64) {
        // 换算函数给出的月份从 1 开始，这里减回去。
        let (year, month, day) = civilFromDays(millis.dividingFloor(DAY_MS))
        self.year = year
        self.month = Int(month) - 1
        self.day = Int(day)
    }

    /// 给出年、月（从 0 开始）与日。
    init(year: Int64, month: Int, day: Int) {
        self.year = year
        self.month = month
        self.day = day
    }

    /// 当天零点的毫秒时间戳。
    var millis: Int64 {
        daysFromCivil(year: year, month: Int64(month) + 1, day: Int64(day)) * DAY_MS
    }
}
