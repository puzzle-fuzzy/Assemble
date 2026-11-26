import Foundation
import Testing
@testable import AssembleCore

/// `Fixtures/recurrence.json`：Rust 那一份算法对各种规则与询问时刻给出的结果。
struct RecurrenceFixture: Decodable {
    struct Ask: Decodable {
        let from: String
        let limit: Int
        let upcoming: [String]
        let latest: String?
    }

    struct Rule: Decodable {
        let name: String
        let startAt: String
        let `repeat`: Repeat
        let repeatUntil: String?
        let asks: [Ask]
    }

    let shanghaiOffsetMs: Int64
    let rules: [Rule]
}

struct RecurrenceTests {
    static let fixture: RecurrenceFixture = {
        let data = try! Data(contentsOf: FixtureDirectory.url.appendingPathComponent("recurrence.json"))
        return try! JSONDecoder().decode(RecurrenceFixture.self, from: data)
    }()

    static func millis(_ text: String) -> Int64 {
        WireTime.parse(text)!
    }

    @Test
    func everyAskMatchesWhatTheRustRulesProduce() {
        #expect(RecurrenceTests.fixture.shanghaiOffsetMs == SHANGHAI_OFFSET_MS)
        #expect(RecurrenceTests.fixture.rules.count >= 70)
        var asks = 0
        for rule in RecurrenceTests.fixture.rules {
            let recurrence = Recurrence(
                startAt: Self.millis(rule.startAt),
                repeating: rule.repeat,
                repeatUntil: rule.repeatUntil.map(Self.millis)
            )
            for ask in rule.asks {
                asks += 1
                let from = Self.millis(ask.from)
                let got = recurrence.upcoming(from: from, limit: ask.limit)
                #expect(got == ask.upcoming.map(Self.millis), "\(rule.name): upcoming from \(ask.from)")
                #expect(recurrence.latestAtOrBefore(from) == ask.latest.map(Self.millis), "\(rule.name): latest before \(ask.from)")
            }
        }
        #expect(asks >= 500)
    }

    @Test
    func theRustUnitTestsHoldHereToo() {
        func beijing(_ year: Int32, _ month: UInt32, _ day: UInt32, _ hour: UInt32, _ minute: UInt32) -> Int64 {
            WireTime.parse(String(format: "%04d-%02d-%02dT%02d:%02d:00.000+08:00", year, month, day, hour, minute))!
        }
        // 只出现一次。
        let start = beijing(2026, 11, 24, 9, 0)
        let once = Recurrence(startAt: start, repeating: .none, repeatUntil: nil)
        #expect(once.latestAtOrBefore(start - 1) == nil)
        #expect(once.latestAtOrBefore(start) == start)
        #expect(once.upcoming(from: start - 1, limit: 10) == [start])
        #expect(once.upcoming(from: start + 1, limit: 10).isEmpty)

        // 月重复留住日号，短月钳制，过后恢复；闰年的二月有 29 日。
        let monthly = Recurrence(startAt: beijing(2026, 1, 31, 9, 0), repeating: .monthly, repeatUntil: nil)
        #expect(monthly.upcoming(from: beijing(2026, 1, 31, 9, 0), limit: 4) == [
            beijing(2026, 1, 31, 9, 0), beijing(2026, 2, 28, 9, 0),
            beijing(2026, 3, 31, 9, 0), beijing(2026, 4, 30, 9, 0),
        ])
        #expect(monthly.latestAtOrBefore(beijing(2028, 3, 1, 0, 0)) == beijing(2028, 2, 29, 9, 0))

        // 重复截止是含的。
        let daily = Recurrence(startAt: start, repeating: .daily, repeatUntil: start + 2 * DAY_MS)
        #expect(daily.latestAtOrBefore(start + 2 * DAY_MS) == start + 2 * DAY_MS)
        #expect(daily.latestAtOrBefore(start + 3 * DAY_MS) == nil)
        #expect(daily.upcoming(from: start, limit: 10) == [start, start + DAY_MS, start + 2 * DAY_MS])

        // 同一时刻的两种写法得到同一条规则。
        let writtenLocally = Recurrence(startAt: WireTime.parse("2026-01-31T09:00:00.000+08:00")!, repeating: .monthly, repeatUntil: nil)
        let writtenInUTC = Recurrence(startAt: WireTime.parse("2026-01-31T01:00:00.000Z")!, repeating: .monthly, repeatUntil: nil)
        #expect(writtenLocally == writtenInUTC)
    }
}
