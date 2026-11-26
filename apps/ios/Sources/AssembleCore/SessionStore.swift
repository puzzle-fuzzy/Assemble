import Foundation
import Security

/// 登录之后这台手机持有的东西：令牌、账户、这台设备。
public struct SignedInSession: Codable, Hashable, Sendable {
    public let token: String
    public let account: Account
    public let device: Device

    public init(token: String, account: Account, device: Device) {
        self.token = token
        self.account = account
        self.device = device
    }
}

/// 登录存在哪里。正式运行用系统钥匙串，测试用内存。
public protocol SessionStoring: Sendable {
    /// 没有保存过的是 `nil`。
    func load() throws -> SignedInSession?
    func save(_ session: SignedInSession) throws
    func clear() throws
}

/// 系统钥匙串。条目名和桌面端同一种规矩：服务名按服务来源区分，账号位是 `current`。
/// 这台手机换不换人用，条目都跟着"哪个服务"走。
public final class KeychainSessionStore: SessionStoring, @unchecked Sendable {
    private let service: String
    private let account = "current"

    public init(origin: URL) {
        service = Self.serviceName(for: origin)
    }

    /// 条目的服务名按服务来源区分，写法和桌面端一样：`assemble.device-session:<来源>`，末尾不带斜杠。
    static func serviceName(for origin: URL) -> String {
        var spelling = origin.absoluteString
        while spelling.count > 1, spelling.hasSuffix("/") {
            spelling.removeLast()
        }
        return "assemble.device-session:\(spelling)"
    }

    private var query: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    public func load() throws -> SignedInSession? {
        var query = query
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        switch status {
        case errSecSuccess:
            guard let data = item as? Data else { throw SessionStoreError.corrupted }
            guard let session = try? JSONDecoder().decode(SignedInSession.self, from: data) else {
                throw SessionStoreError.corrupted
            }
            return session
        case errSecItemNotFound:
            return nil
        default:
            throw SessionStoreError.system(status)
        }
    }

    public func save(_ session: SignedInSession) throws {
        let data = try JSONEncoder().encode(session)
        let attributes: [String: Any] = [
            kSecValueData as String: data,
            // 重启之后、第一次解锁之前也读得到：到点的本地通知不依赖它，但后台刷新要它。
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        var status = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
        if status == errSecItemNotFound {
            var insert = query
            attributes.forEach { insert[$0.key] = $0.value }
            status = SecItemAdd(insert as CFDictionary, nil)
        }
        guard status == errSecSuccess else { throw SessionStoreError.system(status) }
    }

    public func clear() throws {
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw SessionStoreError.system(status)
        }
    }
}

public enum SessionStoreError: Error, Hashable, Sendable {
    case corrupted
    case system(OSStatus)
}

/// 内存里的保存：测试和预览用。
public final class MemorySessionStore: SessionStoring, @unchecked Sendable {
    private var stored: SignedInSession?
    private let lock = NSLock()

    public init(_ session: SignedInSession? = nil) {
        stored = session
    }

    public func load() throws -> SignedInSession? {
        lock.lock()
        defer { lock.unlock() }
        return stored
    }

    public func save(_ session: SignedInSession) throws {
        lock.lock()
        defer { lock.unlock() }
        stored = session
    }

    public func clear() throws {
        lock.lock()
        defer { lock.unlock() }
        stored = nil
    }
}
