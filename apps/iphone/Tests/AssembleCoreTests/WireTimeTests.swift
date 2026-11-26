import Foundation
import Testing
@testable import AssembleCore

struct WireTimeTests {
    @Test
    func parsesTheSpellingTheServerWrites() {
        // "火车"那一条的开始时刻：北京时间 2026-11-27 09:00。
        let millis = WireTime.parse("2026-11-27T01:00:00.000Z")
        #expect(millis == 1_795_741_200_000)
    }

    @Test
    func acceptsOffsetsAndShorterOrLongerFractions() {
        let utc = WireTime.parse("2026-01-31T01:00:00.000Z")
        #expect(WireTime.parse("2026-01-31T09:00:00.000+08:00") == utc)
        #expect(WireTime.parse("2026-01-31T09:00:00+08:00") == utc)
        #expect(WireTime.parse("2026-01-31T01:00:00.5Z") == utc.map { $0 + 500 })
        #expect(WireTime.parse("2026-01-31T01:00:00.123456Z") == utc.map { $0 + 123 })
        #expect(WireTime.parse("2026-01-30T20:00:00.250-05:00") == utc.map { $0 + 250 })
    }

    @Test
    func refusesTextWithoutAZoneOrWithRubbishInIt() {
        for broken in [
            "", "not a date", "2026-01-31T09:00:00", "2026-13-01T08:00:00Z", "2026-02-30T08:00:00Z",
            "2026-01-31 09:00:00Z", "2026-01-31T09:00Z", "2026-01-31T09:00:00.", "2026-01-31T09:00:00Z ",
            "2026-01-31T09:00:00+08:0", "2026-01-31T09:00:00+08:00Z", "2026-01-31T24:00:00Z",
        ] {
            #expect(WireTime.parse(broken) == nil, "\(broken)")
        }
    }

    @Test
    func writesBackExactlyWhatItRead() {
        let fixture = try! String(
            contentsOf: FixtureDirectory.url.appendingPathComponent("protocol.json"),
            encoding: .utf8
        )
        // 样本里出现的每一个时刻，读进来再写出去都应当一字不差。
        var seen = 0
        for token in fixture.split(separator: "\"").map(String.init) {
            guard let millis = WireTime.parse(token) else { continue }
            #expect(WireTime.format(millis) == token)
            seen += 1
        }
        #expect(seen >= 40)
        // 纪元前后与闰年。
        for millis: Int64 in [0, -1, -86_400_001, 951_782_400_000, 1_709_164_800_000] {
            #expect(WireTime.parse(WireTime.format(millis)) == millis)
        }
        #expect(WireTime.format(-1) == "1969-12-31T23:59:59.999Z")
    }
}

/// 样本所在的目录：包根目录下的 `Fixtures/`。
enum FixtureDirectory {
    static let url = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Fixtures")

    static func json(_ name: String) throws -> [String: Any] {
        let data = try Data(contentsOf: url.appendingPathComponent(name))
        return try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
    }
}
