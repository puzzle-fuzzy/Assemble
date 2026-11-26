import Foundation

/// 和服务端说话的那一层。手机应用经它调接口；线上契约在 `crates/assemble-protocol`，
/// 这里的类型和那边一一对应。
///
/// 几条守住的规矩：不带 Cookie、不存凭据、不缓存；不跟随重定向，令牌不会被带到别的站点；
/// 回应有大小上限；令牌只放在 `Authorization` 头里，不进任何返回值之外的地方。
public struct AssembleAPI: Sendable {
    public let origin: URL
    private let session: URLSession
    private let token: String?

    /// 一个回应最多这么大。名单远远用不到。
    public static let maxBodyBytes = 8 * 1024 * 1024
    public static let timeoutSeconds: TimeInterval = 15

    /// `token` 是这台设备的令牌；还没登录时是 `nil`。
    public init(origin: URL, token: String? = nil, session: URLSession = AssembleAPI.makeSession()) {
        self.origin = origin
        self.token = token
        self.session = session
    }

    /// 同一个服务，带着令牌。
    public func signedIn(as token: String) -> AssembleAPI {
        AssembleAPI(origin: origin, token: token, session: session)
    }

    /// 临时配置：不带 Cookie、不存凭据、不缓存，不跟随重定向。
    public static func makeSession(
        configuration: URLSessionConfiguration = .ephemeral
    ) -> URLSession {
        configuration.httpCookieStorage = nil
        configuration.httpShouldSetCookies = false
        configuration.urlCredentialStorage = nil
        configuration.urlCache = nil
        configuration.timeoutIntervalForRequest = timeoutSeconds
        configuration.waitsForConnectivity = false
        return URLSession(configuration: configuration, delegate: RedirectRefuser(), delegateQueue: nil)
    }

    // ---- 扫码配对：不带任何 Cookie 或令牌 ----

    /// `POST /v1/pairing/claim`
    public func claim(_ claim: PairingClaim) async throws -> PairingClaimed {
        try await post(claim, to: "/v1/pairing/claim", as: PairingClaimed.self)
    }

    /// `POST /v1/pairing/finish`
    public func finish(_ finish: PairingFinish) async throws -> PairingOutcome {
        try await post(finish, to: "/v1/pairing/finish", as: PairingOutcome.self)
    }

    // ---- 登录之后 ----

    /// `GET /v1/me`：令牌还管不管用。
    public func me() async throws -> MeResponse {
        try await send("GET", "/v1/me", as: MeResponse.self)
    }

    /// `GET /v1/planner`
    public func planner() async throws -> Planner {
        try await send("GET", "/v1/planner", as: Planner.self)
    }

    /// `POST /v1/notifications/read`
    public func readNotifications(_ ids: [String]) async throws {
        struct Input: Encodable {
            let ids: [String]
        }
        try await post(Input(ids: ids), to: "/v1/notifications/read")
    }

    /// `POST /v1/notifications/read-all`
    public func readAllNotifications() async throws {
        try await send("POST", "/v1/notifications/read-all")
    }

    public func dismissReminder(_ id: String, version: Int64) async throws -> Reminder {
        try await act("dismiss", on: "reminders", id: id, version: version, as: Reminder.self)
    }

    public func cancelReminder(_ id: String, version: Int64) async throws -> Reminder {
        try await act("cancel", on: "reminders", id: id, version: version, as: Reminder.self)
    }

    public func completeSchedule(_ id: String, version: Int64) async throws -> Schedule {
        try await act("complete", on: "schedules", id: id, version: version, as: Schedule.self)
    }

    public func cancelSchedule(_ id: String, version: Int64) async throws -> Schedule {
        try await act("cancel", on: "schedules", id: id, version: version, as: Schedule.self)
    }

    /// `POST /v1/auth/logout`：撤销这台设备的令牌并删除设备记录。
    public func logout() async throws {
        try await send("POST", "/v1/auth/logout")
    }

    // ---- 内部 ----

    private struct Versioned: Encodable {
        let version: Int64
    }

    private func act<Response: Decodable>(
        _ action: String,
        on collection: String,
        id: String,
        version: Int64,
        as type: Response.Type
    ) async throws -> Response {
        guard Self.isID(id) else { throw APIError.invalidRequest }
        return try await post(
            Versioned(version: version),
            to: "/v1/\(collection)/\(id)/\(action)",
            as: type
        )
    }

    /// 标识只由字母、数字和 `_.-` 组成：拼进地址之前先看一眼。
    static func isID(_ id: String) -> Bool {
        !id.isEmpty && id.utf8.count <= 200 && id.utf8.allSatisfy { byte in
            (65...90).contains(byte) || (97...122).contains(byte) || (48...57).contains(byte)
                || byte == 95 || byte == 46 || byte == 45
        }
    }

    private func post<Response: Decodable>(
        _ body: some Encodable,
        to path: String,
        as type: Response.Type
    ) async throws -> Response {
        try await send("POST", path, body: JSONEncoder().encode(body), as: type)
    }

    private func post(_ body: some Encodable, to path: String) async throws {
        try await send("POST", path, body: JSONEncoder().encode(body))
    }

    private func send<Response: Decodable>(
        _ method: String,
        _ path: String,
        body: Data? = nil,
        as type: Response.Type
    ) async throws -> Response {
        let (data, _) = try await perform(method, path, body: body)
        guard let value = try? JSONDecoder().decode(Response.self, from: data) else {
            throw APIError.badResponse
        }
        return value
    }

    private func send(_ method: String, _ path: String, body: Data? = nil) async throws {
        _ = try await perform(method, path, body: body)
    }

    private func perform(_ method: String, _ path: String, body: Data?) async throws -> (Data, Int) {
        var request = URLRequest(url: origin.appending(path: path))
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let body {
            request.httpBody = body
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        if let token {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }

        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: request)
        } catch let error as URLError {
            if error.code == .timedOut {
                throw APIError.timeout
            }
            throw APIError.network(error.code.rawValue)
        } catch is CancellationError {
            throw APIError.cancelled
        }
        guard let http = response as? HTTPURLResponse else {
            throw APIError.badResponse
        }
        // 不跟着走：服务端不该把我们送去别处，真要送就当出错，令牌绝不外带。
        if (300..<400).contains(http.statusCode) {
            throw APIError.redirected
        }
        if data.count > Self.maxBodyBytes {
            throw APIError.tooLarge
        }
        if !(200..<300).contains(http.statusCode) {
            throw Self.failure(status: http.statusCode, data: data)
        }
        return (data, http.statusCode)
    }

    static func failure(status: Int, data: Data) -> APIError {
        failure(status: status, code: (try? JSONDecoder().decode(ServerErrorBody.self, from: data))?.error.code)
    }

    /// 把服务端的稳定错误码翻成这一层的错误。
    public static func failure(status: Int, code: String?) -> APIError {
        switch code {
        case "UNAUTHORIZED": return .unauthorized
        case "INVALID_AUTH_CODE": return .invalidCode
        case "TOO_MANY_ATTEMPTS": return .tooManyAttempts
        case "VERSION_CONFLICT": return .versionConflict
        case "INVALID_STATE": return .invalidState
        case "INVALID_REQUEST": return .invalidRequest
        case "FORBIDDEN": return .forbidden
        default: break
        }
        switch status {
        case 401: return .unauthorized
        case 404: return .notFound
        default: return .http(status: status, code: code)
        }
    }
}

public enum APIError: Error, Hashable, Sendable {
    case network(Int)
    case timeout
    case cancelled
    /// 回应不是这个接口该给的 JSON。
    case badResponse
    case redirected
    case tooLarge
    case unauthorized
    case notFound
    /// 配对的票据不对、过期或者已经用过。
    case invalidCode
    case tooManyAttempts
    /// 那一条已经被别处改过：重新读一遍再说。
    case versionConflict
    case invalidState
    case invalidRequest
    case forbidden
    case http(status: Int, code: String?)

    /// 令牌已经不管用了：该回到登录页。
    public var isAuthenticationFailure: Bool {
        self == .unauthorized
    }
}

/// 拒绝一切重定向。
private final class RedirectRefuser: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
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
