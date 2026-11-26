import Foundation
import Testing
@testable import AssembleCore

/// 不出门的"服务端"：按请求当场给出回应，并记下收到的一切。
final class StubServer: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var answer: (@Sendable (URLRequest, Data?) -> Reply)?
    nonisolated(unsafe) private(set) static var received: [Captured] = []

    struct Reply {
        var status = 200
        var body = Data()
        var headers: [String: String] = [:]

        static func json(_ status: Int, _ text: String) -> Reply {
            Reply(status: status, body: Data(text.utf8), headers: ["Content-Type": "application/json"])
        }
    }

    struct Captured {
        let method: String?
        let url: URL?
        let headers: [String: String]
        let body: Data?
    }

    static func reset() {
        answer = nil
        received = []
    }

    static func last() -> Captured? {
        received.last
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let body = request.httpBody ?? Self.read(request.httpBodyStream)
        Self.received.append(
            Captured(method: request.httpMethod, url: request.url, headers: request.allHTTPHeaderFields ?? [:], body: body)
        )
        guard let answer = Self.answer else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        let reply = answer(request, body)
        let response = HTTPURLResponse(
            url: request.url!, statusCode: reply.status, httpVersion: "HTTP/1.1", headerFields: reply.headers
        )!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: reply.body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}

    private static func read(_ stream: InputStream?) -> Data? {
        guard let stream else { return nil }
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        while stream.hasBytesAvailable {
            let count = stream.read(&buffer, maxLength: buffer.count)
            guard count > 0 else { break }
            data.append(buffer, count: count)
        }
        return data
    }
}

@Suite(.serialized)
struct APITests {
    let origin = URL(string: "https://assemble.example.test")!

    func api(token: String? = nil) -> AssembleAPI {
        StubServer.reset()
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [StubServer.self]
        return AssembleAPI(origin: origin, token: token, session: AssembleAPI.makeSession(configuration: configuration))
    }

    @Test
    func claimingSendsTheProofAndKeepsNoCookiesOrToken() async throws {
        let api = api()
        StubServer.answer = { _, _ in
            .json(200, #"{"confirmationCode":"004757","expiresAt":"2026-11-27T01:03:00.000Z"}"#)
        }
        let claimed = try await api.claim(
            PairingClaim(ticket: "t", codeChallenge: "c", deviceName: "我的 iPhone")
        )
        #expect(claimed.confirmationCode == "004757")
        let request = try #require(StubServer.last())
        #expect(request.method == "POST")
        #expect(request.url?.absoluteString == "https://assemble.example.test/v1/pairing/claim")
        #expect(request.headers["Authorization"] == nil)
        #expect(request.headers["Cookie"] == nil)
        let body = try #require(try JSONSerialization.jsonObject(with: request.body!) as? [String: String])
        #expect(body == ["ticket": "t", "codeChallenge": "c", "deviceName": "我的 iPhone"])
    }

    @Test
    func aFinishedPairingHandsOverTheSessionOnlyWhenComplete() async throws {
        let api = api()
        let fixture = Fixtures.wire
        for (text, state) in [
            (#"{"state":"pending"}"#, PairingState.pending),
            (#"{"state":"denied"}"#, PairingState.denied),
        ] {
            StubServer.answer = { _, _ in .json(200, text) }
            let outcome = try await api.finish(PairingFinish(ticket: "t", codeVerifier: "v"))
            #expect(outcome.state == state)
            #expect(outcome.signedIn == nil)
        }
        StubServer.answer = { _, _ in .json(200, String(data: try! JSONEncoder().encode(fixture.pairingOutcomes.complete), encoding: .utf8)!) }
        let outcome = try await api.finish(PairingFinish(ticket: "t", codeVerifier: "v"))
        #expect(outcome.signedIn?.token == String(repeating: "t", count: 43))
        #expect(outcome.signedIn?.account == fixture.account)
    }

    @Test
    func signedInCallsCarryTheTokenAndNothingElse() async throws {
        let token = String(repeating: "k", count: 43)
        let api = api(token: token)
        StubServer.answer = { _, _ in
            .json(200, String(data: try! JSONEncoder().encode(Fixtures.wire.me), encoding: .utf8)!)
        }
        let me = try await api.me()
        #expect(me.device.platform == .ios)
        let request = try #require(StubServer.last())
        #expect(request.headers["Authorization"] == "Bearer \(token)")
        #expect(request.headers["Accept"] == "application/json")
        #expect(request.headers["Cookie"] == nil)
        #expect(request.headers["Content-Type"] == nil, "没有正文的请求不声明正文")
    }

    @Test
    func actionsCarryTheVersionTheCallerSaw() async throws {
        let api = api(token: "k")
        StubServer.answer = { request, body in
            let body = try! JSONSerialization.jsonObject(with: body!) as! [String: Int64]
            return .json(200, #"{"version":\#(body["version"]! + 1)}"#)
        }
        // 这里只看发出去的东西：返回的 JSON 不是一条完整的提醒，读不出来是预期的。
        await #expect(throws: APIError.badResponse) {
            try await api.dismissReminder("r-1", version: 3)
        }
        let request = try #require(StubServer.last())
        #expect(request.url?.path == "/v1/reminders/r-1/dismiss")
        #expect(request.body == Data(#"{"version":3}"#.utf8))
        #expect(request.headers["Content-Type"] == "application/json")
        await #expect(throws: APIError.badResponse) {
            try await api.completeSchedule("s-1", version: 1)
        }
        #expect(StubServer.last()?.url?.path == "/v1/schedules/s-1/complete")
        // 标识里有别的字符就不发出去：它会被拼进地址。
        await #expect(throws: APIError.invalidRequest) {
            try await api.cancelSchedule("../v1/me", version: 1)
        }
        #expect(StubServer.received.count == 2)
    }

    @Test
    func markingReadSendsTheIdentifiers() async throws {
        let api = api(token: "k")
        StubServer.answer = { _, _ in StubServer.Reply(status: 204) }
        try await api.readNotifications(["a", "b"])
        #expect(StubServer.last()?.body == Data(#"{"ids":["a","b"]}"#.utf8))
        try await api.readAllNotifications()
        #expect(StubServer.last()?.url?.path == "/v1/notifications/read-all")
        try await api.logout()
        #expect(StubServer.last()?.url?.path == "/v1/auth/logout")
    }

    @Test
    func errorsAreToldApartByTheirCode() async throws {
        let api = api(token: "k")
        for sample in Fixtures.wire.errors {
            let body = String(data: try JSONEncoder().encode(sample.body), encoding: .utf8)!
            StubServer.answer = { _, _ in .json(sample.status, body) }
            let expected = AssembleAPI.failure(status: sample.status, code: sample.body.error.code)
            await #expect(throws: expected) {
                _ = try await api.planner()
            }
        }
        // 不是那个形状的正文只剩状态码。
        StubServer.answer = { _, _ in StubServer.Reply(status: 404, body: Data("nope".utf8)) }
        await #expect(throws: APIError.notFound) { _ = try await api.planner() }
        StubServer.answer = { _, _ in StubServer.Reply(status: 503, body: Data("{}".utf8)) }
        await #expect(throws: APIError.http(status: 503, code: nil)) { _ = try await api.planner() }
    }

    @Test
    func aRedirectIsNeverFollowed() async throws {
        let api = api(token: "secret-token")
        StubServer.answer = { _, _ in
            StubServer.Reply(status: 302, body: Data(), headers: ["Location": "https://evil.example.test/v1/planner"])
        }
        await #expect(throws: APIError.redirected) { _ = try await api.planner() }
        #expect(StubServer.received.count == 1, "被送去的那个地址一个请求都没发")
        #expect(StubServer.last()?.url?.host == "assemble.example.test")
    }

    @Test
    func bodiesThatAreTooBigOrNotJSONAreRefused() async throws {
        let api = api(token: "k")
        StubServer.answer = { _, _ in StubServer.Reply(status: 200, body: Data(repeating: 32, count: AssembleAPI.maxBodyBytes + 1)) }
        await #expect(throws: APIError.tooLarge) { _ = try await api.planner() }
        StubServer.answer = { _, _ in .json(200, #"{"schedules":"none"}"#) }
        await #expect(throws: APIError.badResponse) { _ = try await api.planner() }
        StubServer.answer = { _, _ in StubServer.Reply(status: 200, body: Data("<html>".utf8)) }
        await #expect(throws: APIError.badResponse) { _ = try await api.planner() }
    }

    @Test
    func identifiersAreCheckedBeforeTheyReachAnAddress() {
        #expect(AssembleAPI.isID("c4cf4220b94fddee4f2fe490124b3fa9"))
        #expect(AssembleAPI.isID("a.b_c-9"))
        for bad in ["", "a b", "a/b", "a?b", "../x", "é", String(repeating: "a", count: 201)] {
            #expect(!AssembleAPI.isID(bad), "\(bad)")
        }
    }
}
