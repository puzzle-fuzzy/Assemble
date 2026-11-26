import Foundation
import Testing
@testable import AssembleCore

struct PairingTests {
    @Test
    func theChallengeIsTheSha256OfTheVerifier() {
        // RFC 7636 附录 B 的例子。
        #expect(
            PKCE.challenge(for: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")
                == "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        )
    }

    @Test
    func everyGeneratedProofHasTheShapeTheServerAccepts() {
        var seen: Set<String> = []
        for _ in 0..<50 {
            let proof = PKCE.generate()
            #expect(PairingAddress.isProof(proof.verifier))
            #expect(PairingAddress.isProof(proof.challenge))
            #expect(proof.challenge == PKCE.challenge(for: proof.verifier))
            #expect(seen.insert(proof.verifier).inserted, "校验器不能重复")
        }
    }

    @Test
    func onlyTheExactAddressYieldsATicket() {
        let ticket = "abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE"
        let uri = PairingAddress.uri(ticket: ticket)
        #expect(uri == "assemble://pair?ticket=\(ticket)")
        #expect(PairingAddress.ticket(from: uri) == ticket)
        // 粘贴进来的时候两头常带着空白。
        #expect(PairingAddress.ticket(from: "  \(uri)\n") == ticket)
        for other in [
            "https://pair?ticket=\(ticket)",
            "assemble://other?ticket=\(ticket)",
            "assemble://pair?ticket=\(ticket)&next=/admin",
            "assemble://pair?tickets=\(ticket)",
            "assemble://pair",
            "assemble://pair?ticket=short",
            "",
        ] {
            #expect(PairingAddress.ticket(from: other) == nil, "\(other)")
        }
    }

    @Test
    func theProofShapeIsPinnedToBase64urlWithoutPadding() {
        #expect(PairingAddress.isProof(String(repeating: "a", count: 43)))
        #expect(!PairingAddress.isProof(String(repeating: "a", count: 42)))
        #expect(!PairingAddress.isProof(String(repeating: "a", count: 44)))
        #expect(!PairingAddress.isProof(String(repeating: "+", count: 43)))
        #expect(!PairingAddress.isProof(String(repeating: "=", count: 43)))
    }

    @Test
    func theSessionStoreKeepsWhatItWasGiven() throws {
        let store = MemorySessionStore()
        #expect(try store.load() == nil)
        let session = Fixtures.wire.pairingOutcomes.complete.signedIn!
        try store.save(session)
        #expect(try store.load() == session)
        try store.clear()
        #expect(try store.load() == nil)
        try store.clear()
    }

    @Test
    func theKeychainEntryIsNamedAfterTheServiceLikeOnTheDesktop() {
        // 这里只看条目的名字，不去碰系统钥匙串。
        func name(_ origin: String) -> String {
            KeychainSessionStore.serviceName(for: URL(string: origin)!)
        }
        #expect(name("https://as.example.test") == "assemble.device-session:https://as.example.test")
        #expect(name("https://as.example.test/") == "assemble.device-session:https://as.example.test")
        #expect(name("http://127.0.0.1:8787") == "assemble.device-session:http://127.0.0.1:8787")
    }
}
