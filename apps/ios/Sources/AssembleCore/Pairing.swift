import CryptoKit
import Foundation
import Security

/// 扫码配对里手机这一侧的东西：PKCE 和二维码里的地址。
public enum PKCE {
    public struct Proof: Hashable, Sendable {
        /// 只留在手机上；取令牌的时候才交出去。
        public let verifier: String
        /// 交给服务端的挑战：校验器的 SHA-256。
        public let challenge: String
    }

    /// 生成一对校验器与挑战。校验器是 32 个随机字节的 base64url，不带填充，正好 43 个字符。
    public static func generate() -> Proof {
        var bytes = [UInt8](repeating: 0, count: 32)
        let status = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        precondition(status == errSecSuccess, "the system could not provide random bytes")
        let verifier = Self.base64url(bytes)
        return Proof(verifier: verifier, challenge: challenge(for: verifier))
    }

    /// S256：挑战是校验器的 SHA-256，再写成 base64url。
    public static func challenge(for verifier: String) -> String {
        base64url(Array(SHA256.hash(data: Data(verifier.utf8))))
    }

    /// 不带填充的 base64url：线上形式的字母表。
    static func base64url(_ bytes: [UInt8]) -> String {
        Data(bytes).base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }
}

/// 二维码里的地址：`assemble://pair?ticket=<票据>`。票据和 PKCE 的校验器是同一种形式。
public enum PairingAddress {
    public static let scheme = "assemble"
    public static let host = "pair"
    static let prefix = "\(scheme)://\(host)?ticket="

    /// 把票据写成地址。
    public static func uri(ticket: String) -> String {
        "\(prefix)\(ticket)"
    }

    /// 从一个地址里取出票据。扫到的结果、或者用户粘贴进来的一段文字都走这里。
    /// 只认这一个形状：别的地址、多出来的参数、不合规的票据都算扫错了。
    public static func ticket(from uri: String) -> String? {
        let trimmed = uri.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let ticket = trimmed.hasPrefix(prefix) ? String(trimmed.dropFirst(prefix.count)) : nil,
              isProof(ticket)
        else { return nil }
        return ticket
    }

    /// 票据、校验器、挑战的线上形式：不带填充的 base64url，恰好 43 个字符。
    public static func isProof(_ value: String) -> Bool {
        value.utf8.count == 43 && value.utf8.allSatisfy { byte in
            (65...90).contains(byte) || (97...122).contains(byte) || (48...57).contains(byte)
                || byte == 45 || byte == 95
        }
    }
}
