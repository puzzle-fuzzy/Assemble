import CryptoKit
import Foundation
import XCTest

/// 在模拟器里真的把应用跑起来，对着本机一次性起的服务端走完：
/// 扫码配对登录 → 名单 → 打开到点提醒 → 切到后台 → 系统通知弹出 → 点它定位到那一条 → 标成看过了。
///
/// "电脑那一侧"由测试程序自己扮演：用测试账户登录网页、生成二维码、点允许，再用一台
/// "桌面设备"的令牌往账户里放一条日程和一条提醒。需要的东西经环境变量交给测试进程
/// （xcodebuild 会把 `TEST_RUNNER_` 开头的变量转过来）：`ASSEMBLE_ORIGIN`、`ASSEMBLE_EMAIL`、`ASSEMBLE_PASSWORD`。
@MainActor
final class PairingFlowUITests: XCTestCase {
    private var web: WebSide!
    private let app = XCUIApplication()
    private let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")

    override func setUpWithError() throws {
        continueAfterFailure = false
        // xcodebuild 把命令行上 `TEST_RUNNER_` 开头的变量交给测试进程；名字带不带前缀都认。
        func setting(_ name: String) -> String? {
            let environment = ProcessInfo.processInfo.environment
            return environment[name] ?? environment["TEST_RUNNER_\(name)"]
        }
        let origin = setting("ASSEMBLE_ORIGIN") ?? "http://127.0.0.1:8792"
        web = WebSide(
            origin: URL(string: origin)!,
            email: setting("ASSEMBLE_EMAIL") ?? "iphonetest@example.test",
            password: try XCTUnwrap(setting("ASSEMBLE_PASSWORD"), "ASSEMBLE_PASSWORD must reach the test process")
        )
        app.launchEnvironment["ASSEMBLE_SERVER_ORIGIN"] = origin
    }

    override func tearDownWithError() throws {
        app.terminate()
    }

    func testPairingSignInRemindersAndOpeningANotification() async throws {
        // ---- "电脑"：登录网页，生成二维码，再往账户里放一条日程和一条提醒 ----
        try await web.signIn()
        let (pairingID, address) = try await web.newPairingCode()
        let stamp = Int64(Date().timeIntervalSince1970 * 1000)
        let desktop = try await web.registerDesktop()
        try await web.createSchedule(token: desktop, title: "iOS 验收日程", startAt: stamp + 2 * 86_400_000)

        // ---- "手机"：粘贴配对地址，认领，显示六位数字 ----
        app.launch()
        XCTAssertTrue(app.buttons["signin.scan"].waitForExistence(timeout: 15), "登录页要出来")
        app.buttons["signin.paste"].tap()
        // 多行的输入框在界面测试里是哪一种元素不完全固定，按标识找。
        let field = app.descendants(matching: .any).matching(identifier: "pairing.address").firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 10), "粘贴框要出来")
        field.tap()
        field.typeText(address)
        let go = app.buttons["pairing.continue"]
        XCTAssertTrue(go.waitForExistence(timeout: 5))
        go.tap()
        let code = app.staticTexts["pairing.code"]
        XCTAssertTrue(code.waitForExistence(timeout: 20), "认领之后要显示六位数字")
        XCTAssertEqual(code.label.count, 6, "显示的是一个六位数字：\(code.label)")
        XCTAssertTrue(code.label.allSatisfy(\.isNumber))
        keepScreenshot("01-认领之后显示六位数字")
        XCTAssertTrue(app.buttons["signin.scan"].exists, "本人点了允许之前不登录")

        // ---- "电脑"：核对之后点允许 ----
        try await web.approve(pairingID)
        XCTAssertTrue(app.staticTexts["iOS 验收日程"].waitForExistence(timeout: 30), "登录之后名单里要有那条日程")
        keepScreenshot("02-登录之后的名单")

        // 登录之后再放一条一分半之后到点的提醒：它得在"打开到点提醒"之后才到点。
        let dueAt = Int64(Date().timeIntervalSince1970 * 1000) + 100_000
        try await web.createReminder(token: desktop, title: "iOS 验收提醒", dueAt: dueAt)
        XCTAssertTrue(app.staticTexts["iOS 验收提醒"].waitForExistence(timeout: 40), "名单里要有那条提醒")

        // ---- 打开到点提醒（系统会问一次）----
        app.buttons["home.account"].tap()
        let toggle = app.switches["account.reminders"]
        XCTAssertTrue(toggle.waitForExistence(timeout: 10))
        // 这一行中间是空的：点右端那颗真的开关。点在行中间不会切换（2026-11-27 试过两回，开关纹丝不动）。
        toggle.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
        keepScreenshot("02a-点了到点提醒的开关")
        // 系统问"允许通知吗"的时候替用户点允许；点了之后开关才会变成开。
        let deadline = Date().addingTimeInterval(30)
        var turnedOn = false
        while Date() < deadline && !turnedOn {
            allowNotificationPermissionIfAsked()
            turnedOn = Self.isOn(toggle)
            if !turnedOn {
                try await Task.sleep(for: .milliseconds(500))
            }
        }
        XCTAssertTrue(turnedOn, "打开开关之后它应当是开的（读到的是 \(String(describing: toggle.value))）")
        // 异步的测试里断言失败不会自己停：后面每一步都依赖这一步，停在这里，别带着失败往下走。
        guard turnedOn else { throw WebSide.Failure(description: "the reminders switch did not turn on") }
        keepScreenshot("02b-打开了到点提醒")
        app.buttons["完成"].tap()
        XCTAssertTrue(app.buttons["home.account"].waitForExistence(timeout: 5))

        // ---- 切到后台，等系统通知 ----
        XCUIDevice.shared.press(.home)
        let banner = springboard.descendants(matching: .any)
            .matching(NSPredicate(format: "label CONTAINS %@", "iOS 验收提醒"))
            .firstMatch
        let found = banner.waitForExistence(timeout: 190)
        XCTAssertTrue(found, "到点之后要有系统通知")
        keepScreenshot("03-到点之后的系统通知")
        guard found else { throw WebSide.Failure(description: "no notification banner appeared") }
        // 横幅刚出现的那一瞬还在做入场的动画，这时候点它不算。等它停稳再点。
        try await Task.sleep(for: .seconds(1.5))
        banner.tap()
        // 点横幅把应用带到前台。没带起来、横幅还在的话再点一次。
        var inFront = app.wait(for: .runningForeground, timeout: 10)
        if !inFront, banner.exists {
            banner.tap()
            inFront = app.wait(for: .runningForeground, timeout: 10)
        }
        XCTAssertTrue(inFront, "点开通知要把应用带到前台")
        guard inFront else { throw WebSide.Failure(description: "tapping the notification did not bring the app to the front") }

        // ---- 点开通知：定位到那一条提醒 ----
        let dismiss = app.buttons["看过了"]
        let located = dismiss.waitForExistence(timeout: 20)
        XCTAssertTrue(located, "点开通知要定位到那条提醒")
        keepScreenshot("04-点开通知定位到那一条")
        guard located else { throw WebSide.Failure(description: "opening the notification did not locate the reminder") }
        dismiss.tap()

        // ---- 标成看过了：详情自己收起来，名单里那一条不在了 ----
        let detail = app.navigationBars["iOS 验收提醒"]
        let closed = expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: detail)
        await fulfillment(of: [closed], timeout: 15)
        XCTAssertFalse(detail.exists, "看过了之后详情要收起来")
        let row = app.staticTexts["iOS 验收提醒"]
        let removed = expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: row)
        await fulfillment(of: [removed], timeout: 15)
        keepScreenshot("05-看过了之后")
        XCTAssertFalse(row.exists, "看过了之后名单里不再有这一条")

        // 服务端到点之后十几秒之内才为这条提醒生成通知。等它一定生成过了，再让手机读一次名单：
        // 办完的那一条，它迟到的通知也要算看过，不能在"没读的通知"里再冒出来。
        let generatedBy = Date(timeIntervalSince1970: TimeInterval(dueAt) / 1000 + 12)
        let wait = generatedBy.timeIntervalSinceNow
        if wait > 0 {
            try await Task.sleep(for: .seconds(wait))
        }
        // 下拉刷新。SwiftUI 的 List 在这一版系统里不是 Table，按整个应用来滑。
        app.swipeDown()
        let stayedAway = expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: row)
        await fulfillment(of: [stayedAway], timeout: 15)
        try await Task.sleep(for: .seconds(3))
        keepScreenshot("06-它迟到的通知来了之后")
        XCTAssertFalse(row.exists, "它迟到的通知不能再出现在名单里")

        // ---- 服务端那一边：那条提醒确实标成了看过了 ----
        let planner = try await web.planner(token: desktop)
        let reminders = try XCTUnwrap(planner["reminders"] as? [[String: Any]])
        let reminder = try XCTUnwrap(reminders.first { ($0["title"] as? String) == "iOS 验收提醒" })
        XCTAssertEqual(reminder["status"] as? String, "dismissed")
    }

    /// 留一张截图在测试报告里：看完之后可以去核对界面。截的是整块屏幕——
    /// 应用在后台的时候截它自己是黑的。
    private func keepScreenshot(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    /// 开关的读数。开的时候是 1，可它是数字还是文字随系统版本变过，所以按它的描述认。
    private static func isOn(_ toggle: XCUIElement) -> Bool {
        let text = String(describing: toggle.value ?? "").lowercased()
        return text == "1" || text == "on" || text == "开"
    }

    /// 系统问"允许通知吗"的时候替用户点允许。这个框可能挂在桌面的名下，也可能挂在应用的名下，两边都找。
    private func allowNotificationPermissionIfAsked() {
        for alert in [springboard.alerts.firstMatch, app.alerts.firstMatch] where alert.exists {
            for title in ["允许", "Allow", "OK", "好"] {
                let button = alert.buttons[title]
                if button.exists {
                    button.tap()
                    return
                }
            }
        }
    }
}

/// 扮演"电脑那一侧"：登录网页、生成并批准配对、用一台桌面设备的令牌调接口。
final class WebSide: @unchecked Sendable {
    struct Failure: Error, CustomStringConvertible {
        let description: String
    }

    private let origin: URL
    private let email: String
    private let password: String
    private let session: URLSession
    private var csrf = ""

    init(origin: URL, email: String, password: String) {
        self.origin = origin
        self.email = email
        self.password = password
        // 会存 Cookie（这是"已登录的浏览器"），但不跟随重定向：跳到哪里正是要读的东西。
        session = URLSession(configuration: .ephemeral, delegate: RedirectStopper(), delegateQueue: nil)
    }

    private func request(_ method: String, _ path: String, body: Data? = nil, headers: [String: String] = [:]) -> URLRequest {
        // 直接拼：`appending(path:)` 会把查询串里的问号当成路径的一部分转义掉。
        var request = URLRequest(url: URL(string: origin.absoluteString + path)!)
        request.httpMethod = method
        request.setValue(origin.absoluteString, forHTTPHeaderField: "Origin")
        if let body {
            request.httpBody = body
        }
        for (name, value) in headers {
            request.setValue(value, forHTTPHeaderField: name)
        }
        return request
    }

    private func send(_ request: URLRequest) async throws -> (String, HTTPURLResponse) {
        let (data, response) = try await session.data(for: request)
        return (String(decoding: data, as: UTF8.self), try XCTUnwrap(response as? HTTPURLResponse))
    }

    private func form(_ path: String, _ fields: [(String, String)]) async throws -> (String, HTTPURLResponse) {
        var allowed = CharacterSet.alphanumerics
        allowed.insert(charactersIn: "-._*")
        let body = fields
            .map { "\($0.0)=\($0.1.addingPercentEncoding(withAllowedCharacters: allowed) ?? "")" }
            .joined(separator: "&")
        return try await send(
            request("POST", path, body: Data(body.utf8), headers: ["Content-Type": "application/x-www-form-urlencoded"])
        )
    }

    private func readCSRF(from page: String) throws -> String {
        let pattern = ##"name="csrf" value="([^"]+)""##
        let regex = try NSRegularExpression(pattern: pattern)
        guard let match = regex.firstMatch(in: page, range: NSRange(page.startIndex..., in: page)),
              let range = Range(match.range(at: 1), in: page)
        else { throw Failure(description: "the page has no csrf field") }
        return String(page[range])
    }

    func signIn() async throws {
        let (page, _) = try await send(request("GET", "/web/login"))
        csrf = try readCSRF(from: page)
        let (answer, response) = try await form("/web/login", [("email", email), ("password", password), ("next", "/web/account"), ("csrf", csrf)])
        guard response.statusCode == 303, response.value(forHTTPHeaderField: "Location") == "/web/account" else {
            throw Failure(description: "signing in on the web answered \(response.statusCode) → \(response.value(forHTTPHeaderField: "Location") ?? "nowhere"): \(answer.prefix(600))")
        }
    }

    /// 生成一张二维码，交出这一次配对的 id 和页面上的那一串地址。
    func newPairingCode() async throws -> (String, String) {
        let (page, _) = try await send(request("GET", "/web/pairing"))
        csrf = try readCSRF(from: page)
        let (_, response) = try await form("/web/pairing", [("csrf", csrf)])
        XCTAssertEqual(response.statusCode, 303)
        let location = try XCTUnwrap(response.value(forHTTPHeaderField: "Location"))
        let id = try XCTUnwrap(location.split(separator: "=").last.map(String.init))
        let (shown, shownResponse) = try await send(request("GET", location))
        let marker = "break-all text-sm select-all\">"
        guard let start = shown.range(of: marker)?.upperBound, let end = shown[start...].firstIndex(of: "<") else {
            throw Failure(description: "the pairing page does not show the address; location was \(location), answered \(shownResponse.statusCode): \(shown.prefix(300))")
        }
        return (id, String(shown[start..<end]))
    }

    func approve(_ id: String) async throws {
        let (page, _) = try await send(request("GET", "/web/pairing"))
        csrf = try readCSRF(from: page)
        let (_, response) = try await form("/web/pairing/\(id)", [("csrf", csrf), ("choice", "allow")])
        XCTAssertEqual(response.statusCode, 303)
    }

    /// 替一台桌面设备走"授权 → 换码"，拿到它的令牌。回调地址上没有人听，授权码从跳转地址里读。
    func registerDesktop() async throws -> String {
        let verifier = Self.random()
        let state = Self.random()
        let challenge = Self.base64url(Data(SHA256.hash(data: Data(verifier.utf8))))
        let redirect = "http://127.0.0.1:54321/callback"
        let device: [String: Any] = ["name": "验收用的电脑", "platform": "macos"]
        let authorization: [String: Any] = [
            "redirectUri": redirect, "codeChallenge": challenge, "state": state, "device": device,
        ]
        let encoded = Self.base64url(try JSONSerialization.data(withJSONObject: authorization))
        let (page, _) = try await send(request("GET", "/desktop/authorize?request=\(encoded)"))
        csrf = try readCSRF(from: page)
        let (_, response) = try await form("/desktop/authorize", [("request", encoded), ("csrf", csrf), ("choice", "allow")])
        XCTAssertEqual(response.statusCode, 303)
        let location = try XCTUnwrap(response.value(forHTTPHeaderField: "Location"))
        guard let code = URLComponents(string: location)?.queryItems?.first(where: { $0.name == "code" })?.value else {
            throw Failure(description: "the redirect carries no code: \(location)")
        }
        let exchange: [String: Any] = ["code": code, "codeVerifier": verifier, "redirectUri": redirect, "device": device]
        let (answer, response2) = try await send(
            request("POST", "/v1/auth/desktop/exchange", body: try JSONSerialization.data(withJSONObject: exchange), headers: ["Content-Type": "application/json"])
        )
        guard response2.statusCode == 200 else { throw Failure(description: "exchange answered \(response2.statusCode): \(answer)") }
        let json = try XCTUnwrap(try JSONSerialization.jsonObject(with: Data(answer.utf8)) as? [String: Any])
        return try XCTUnwrap(json["token"] as? String)
    }

    private func call(_ method: String, _ path: String, token: String, body: [String: Any]? = nil) async throws -> [String: Any] {
        let headers = ["Authorization": "Bearer \(token)", "Content-Type": "application/json"]
        let (text, response) = try await send(
            request(method, path, body: try body.map { try JSONSerialization.data(withJSONObject: $0) }, headers: headers)
        )
        guard (200..<300).contains(response.statusCode) else {
            throw Failure(description: "\(method) \(path) answered \(response.statusCode): \(text)")
        }
        return (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any] ?? [:]
    }

    func createSchedule(token: String, title: String, startAt: Int64) async throws {
        _ = try await call("POST", "/v1/schedules", token: token, body: ["title": title, "startAt": Self.iso(startAt)])
    }

    func createReminder(token: String, title: String, dueAt: Int64) async throws {
        _ = try await call("POST", "/v1/reminders", token: token, body: ["title": title, "dueAt": Self.iso(dueAt)])
    }

    func planner(token: String) async throws -> [String: Any] {
        try await call("GET", "/v1/planner", token: token)
    }

    private static func random() -> String {
        base64url(Data((0..<32).map { _ in UInt8.random(in: 0...255) }))
    }

    private static func base64url(_ data: Data) -> String {
        data.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    private static func iso(_ millis: Int64) -> String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter.string(from: Date(timeIntervalSince1970: TimeInterval(millis) / 1000))
    }
}

/// 不跟随重定向。
final class RedirectStopper: NSObject, URLSessionTaskDelegate {
    func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        willPerformHTTPRedirection response: HTTPURLResponse,
        newRequest request: URLRequest,
        completionHandler: @escaping (URLRequest?) -> Void
    ) {
        completionHandler(nil)
    }
}
