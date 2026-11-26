import Foundation

/// 线上的时间：UTC 的 RFC 3339，精确到毫秒。这里只按整数毫秒保存和比较，不经时区，
/// 所以重复规则的推算和服务端逐位一致。
///
/// 解析是手写的，不用 `ISO8601DateFormatter`：那一个要么只认带小数秒的，要么只认不带的，
/// 而读进来的时间可能来自更新或更旧的服务端。
public struct WireTime: Hashable, Sendable, Codable, Comparable {
    public let millis: Int64

    public init(millis: Int64) {
        self.millis = millis
    }

    /// 给界面显示用的时刻。
    public var date: Date {
        Date(timeIntervalSince1970: TimeInterval(millis) / 1000)
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        let text = try container.decode(String.self)
        guard let millis = WireTime.parse(text) else {
            throw DecodingError.dataCorruptedError(
                in: container,
                debugDescription: "the value is not an RFC 3339 time with a zone"
            )
        }
        self.init(millis: millis)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(WireTime.format(millis))
    }

    public static func < (lhs: WireTime, rhs: WireTime) -> Bool {
        lhs.millis < rhs.millis
    }

    /// 解析 `YYYY-MM-DDTHH:MM:SS[.小数](Z|±HH:MM)`。任何一处不对就是 `nil`。
    public static func parse(_ text: String) -> Int64? {
        guard text.count >= 20, text.count <= 40 else { return nil }
        let characters = Array(text.utf8)
        func digits(_ range: Range<Int>) -> Int64? {
            guard range.upperBound <= characters.count else { return nil }
            var value: Int64 = 0
            for index in range {
                guard characters[index] >= 48, characters[index] <= 57 else { return nil }
                value = value * 10 + Int64(characters[index] - 48)
            }
            return value
        }
        let year = digits(0..<4)
        let month = digits(5..<7)
        let day = digits(8..<10)
        let hour = digits(11..<13)
        let minute = digits(14..<16)
        let second = digits(17..<19)
        guard characters[4] == 45, characters[7] == 45, characters[10] == 84,
              characters[13] == 58, characters[16] == 58,
              let year, let month, let day, let hour, let minute, let second,
              (1...12).contains(month), (1...31).contains(day),
              day <= daysInMonth(year: year, month: month),
              hour <= 23, minute <= 59, second <= 60
        else { return nil }

        var index = 19
        var fraction = Int64(0)
        if index < characters.count, characters[index] == 46 {
            index += 1
            var place = Int64(100)
            var seen = 0
            while index < characters.count, characters[index] >= 48, characters[index] <= 57 {
                if place > 0 {
                    fraction += Int64(characters[index] - 48) * place
                    place /= 10
                }
                index += 1
                seen += 1
            }
            guard seen > 0 else { return nil }
        }

        var offset = Int64(0)
        if index < characters.count, characters[index] == 90 {
            index += 1
        } else if index < characters.count - 5, characters[index] == 43 || characters[index] == 45 {
            let sign = Int64(characters[index] == 43 ? 1 : -1)
            index += 1
            guard characters[index + 2] == 58,
                  let offsetHour = digits(index..<(index + 2)),
                  let offsetMinute = digits((index + 3)..<(index + 5)),
                  offsetHour <= 23, offsetMinute <= 59
            else { return nil }
            offset = sign * (offsetHour * 3600 + offsetMinute * 60)
            index += 5
        } else {
            return nil
        }
        guard index == characters.count else { return nil }

        let days = daysFromCivil(year: year, month: month, day: day)
        let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset
        return seconds * 1000 + fraction
    }

    /// 写成服务端那一种写法：UTC，三位小数。
    public static func format(_ millis: Int64) -> String {
        let total = millis.dividingFloor(1000)
        let fraction = millis.moduloFloor(1000)
        let (year, month, day) = civilFromDays(total.dividingFloor(86_400))
        let clock = total.moduloFloor(86_400)
        func pad(_ value: Int64, _ width: Int) -> String {
            let text = String(abs(value))
            return String(repeating: "0", count: max(0, width - text.count)) + text
        }
        let sign = year < 0 ? "-" : ""
        return "\(sign)\(pad(year, 4))-\(pad(month, 2))-\(pad(day, 2))T\(pad(clock / 3600, 2)):\(pad(clock % 3600 / 60, 2)):\(pad(clock % 60, 2)).\(pad(fraction, 3))Z"
    }
}

extension Int64 {
    /// 向下取整的除法，和 Rust 的 `div_euclid` 一致：负数也向负无穷方向取整。
    public func dividingFloor(_ by: Int64) -> Int64 {
        let quotient = self / by
        let remainder = self % by
        return remainder != 0 && (remainder < 0) != (by < 0) ? quotient - 1 : quotient
    }

    /// 向下取整的余数，和 Rust 的 `rem_euclid` 一致：结果和除数同号。
    public func moduloFloor(_ by: Int64) -> Int64 {
        let remainder = self % by
        return remainder != 0 && (remainder < 0) != (by < 0) ? remainder + by : remainder
    }
}

/// 某年某月有多少天。月份从 1 开始。
func daysInMonth(year: Int64, month: Int64) -> Int64 {
    switch month {
    case 1, 3, 5, 7, 8, 10, 12: 31
    case 4, 6, 9, 11: 30
    default: year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) ? 29 : 28
    }
}

/// 公历日期转自 1970-01-01 起的天数（Howard Hinnant 的算法）。月份从 1 开始。
func daysFromCivil(year: Int64, month: Int64, day: Int64) -> Int64 {
    let shiftedYear = month <= 2 ? year - 1 : year
    let era = shiftedYear.dividingFloor(400)
    let yearOfEra = shiftedYear - era * 400
    let shiftedMonth = month > 2 ? month - 3 : month + 9
    let dayOfYear = (153 * shiftedMonth + 2) / 5 + day - 1
    let dayOfEra = yearOfEra * 365 + yearOfEra / 4 - yearOfEra / 100 + dayOfYear
    return era * 146_097 + dayOfEra - 719_468
}

/// 自 1970-01-01 起的天数转公历日期。月份从 1 开始。
func civilFromDays(_ days: Int64) -> (year: Int64, month: Int64, day: Int64) {
    let shifted = days + 719_468
    let era = shifted.dividingFloor(146_097)
    let dayOfEra = shifted - era * 146_097
    let yearOfEra = (dayOfEra - dayOfEra / 1_460 + dayOfEra / 36_524 - dayOfEra / 146_096) / 365
    let year = yearOfEra + era * 400
    let dayOfYear = dayOfEra - (365 * yearOfEra + yearOfEra / 4 - yearOfEra / 100)
    let monthPointer = (5 * dayOfYear + 2) / 153
    let day = dayOfYear - (153 * monthPointer + 2) / 5 + 1
    let month = monthPointer < 10 ? monthPointer + 3 : monthPointer - 9
    return (month <= 2 ? year + 1 : year, month, day)
}
