import CryptoKit
import Foundation
import LocalAuthentication
import Security

/// Guards the 32-byte database key (architecture §6.6).
///
/// The key is generated on first launch and wrapped with a Secure Enclave P-256 key (ECIES,
/// `.eciesEncryptionCofactorVariableIVX963SHA256AESGCM`); only the wrapped bytes are stored, in the
/// Keychain, never synchronized. With app lock on, the Secure Enclave key requires Touch ID or the
/// Mac password before it can unwrap anything. Destroying the Secure Enclave key makes the
/// database unreadable at once (panic wipe).
///
/// Builds that can't use the Secure Enclave (no signing identity, older hardware) keep the key in
/// the login keychain instead, and `protection` says so.
public final class KeyVault: @unchecked Sendable {
    public enum Protection: Sendable, Equatable {
        /// Wrapped by a key that never leaves the Secure Enclave.
        case secureEnclave
        /// Stored in the login keychain (no Secure Enclave available to this build).
        case keychain
    }

    public enum VaultError: Error, LocalizedError {
        case cancelled
        case failed(String)

        public var errorDescription: String? {
            switch self {
            case .cancelled: "Unlocking was cancelled."
            case let .failed(why): why
            }
        }
    }

    private let service: String
    private let wrapTag: Data
    private static let wrappedAccount = "database-key.wrapped"
    private static let plainAccount = "database-key"
    private static let algorithm = SecKeyAlgorithm.eciesEncryptionCofactorVariableIVX963SHA256AESGCM

    public init(service: String = "com.collinsadi.asiri") {
        self.service = service
        wrapTag = Data("\(service).wrapping-key".utf8)
    }

    /// How the key is protected right now (`nil` before first launch).
    public var protection: Protection? {
        if readItem(account: Self.wrappedAccount) != nil { return .secureEnclave }
        if readItem(account: Self.plainAccount) != nil { return .keychain }
        return nil
    }

    /// Whether a database key exists (the app has been set up on this Mac).
    public var exists: Bool { protection != nil }

    /// Returns the database key, creating and protecting it on first launch. May show the
    /// system's Touch ID prompt when app lock is on.
    public func databaseKey(reason: String) throws -> Data {
        if let wrapped = readItem(account: Self.wrappedAccount) {
            return try unwrap(wrapped, reason: reason)
        }
        if let plain = readItem(account: Self.plainAccount) {
            guard plain.count == 32 else { throw VaultError.failed("The stored database key is damaged.") }
            return plain
        }
        var key = Data(count: 32)
        let status = key.withUnsafeMutableBytes { SecRandomCopyBytes(kSecRandomDefault, 32, $0.baseAddress!) }
        guard status == errSecSuccess else { throw VaultError.failed("Couldn't generate a key (\(status)).") }
        try store(key, appLock: false)
        return key
    }

    /// Turns app lock on or off by re-wrapping the database key with a new Secure Enclave key.
    public func setAppLock(_ enabled: Bool, reason: String) throws {
        let key = try databaseKey(reason: reason)
        deleteWrappingKey()
        deleteItem(account: Self.wrappedAccount)
        deleteItem(account: Self.plainAccount)
        try store(key, appLock: enabled)
    }

    /// Crypto-shreds the database: deletes the Secure Enclave key and every stored copy.
    public func destroy() {
        deleteWrappingKey()
        deleteItem(account: Self.wrappedAccount)
        deleteItem(account: Self.plainAccount)
    }

    // MARK: Wrapping

    private func store(_ key: Data, appLock: Bool) throws {
        if let wrapping = try? createWrappingKey(appLock: appLock),
           let publicKey = SecKeyCopyPublicKey(wrapping) {
            var error: Unmanaged<CFError>?
            if let wrapped = SecKeyCreateEncryptedData(publicKey, Self.algorithm, key as CFData, &error) as Data? {
                try writeItem(wrapped, account: Self.wrappedAccount)
                return
            }
            deleteWrappingKey()
        }
        // No Secure Enclave for this build: the login keychain still keeps it off disk in clear.
        try writeItem(key, account: Self.plainAccount)
    }

    private func createWrappingKey(appLock: Bool) throws -> SecKey {
        var error: Unmanaged<CFError>?
        let flags: SecAccessControlCreateFlags = appLock ? [.privateKeyUsage, .userPresence] : [.privateKeyUsage]
        guard let access = SecAccessControlCreateWithFlags(kCFAllocatorDefault, kSecAttrAccessibleWhenUnlockedThisDeviceOnly, flags, &error) else {
            throw VaultError.failed("Couldn't set up key protection.")
        }
        let attributes: [String: Any] = [
            kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
            kSecAttrKeySizeInBits as String: 256,
            kSecAttrTokenID as String: kSecAttrTokenIDSecureEnclave,
            kSecUseDataProtectionKeychain as String: true,
            kSecPrivateKeyAttrs as String: [
                kSecAttrIsPermanent as String: true,
                kSecAttrApplicationTag as String: wrapTag,
                kSecAttrAccessControl as String: access,
            ] as [String: Any],
        ]
        guard let key = SecKeyCreateRandomKey(attributes as CFDictionary, &error) else {
            throw VaultError.failed("The Secure Enclave isn't available.")
        }
        return key
    }

    private func wrappingKey(reason: String) throws -> SecKey {
        let context = LAContext()
        context.localizedReason = reason
        let query: [String: Any] = [
            kSecClass as String: kSecClassKey,
            kSecAttrApplicationTag as String: wrapTag,
            kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
            kSecUseDataProtectionKeychain as String: true,
            kSecReturnRef as String: true,
            kSecUseAuthenticationContext as String: context,
        ]
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        guard status == errSecSuccess, let item, CFGetTypeID(item) == SecKeyGetTypeID() else {
            throw VaultError.failed("The key that protects your messages is missing. Asiri can't open them on this Mac.")
        }
        return item as! SecKey // swiftlint:disable:this force_cast
    }

    private func unwrap(_ wrapped: Data, reason: String) throws -> Data {
        let key = try wrappingKey(reason: reason)
        var error: Unmanaged<CFError>?
        guard let plain = SecKeyCreateDecryptedData(key, Self.algorithm, wrapped as CFData, &error) as Data? else {
            if let e = error?.takeRetainedValue(), CFErrorGetCode(e) == Int(errSecUserCanceled) || CFErrorGetCode(e) == LAError.userCancel.rawValue {
                throw VaultError.cancelled
            }
            throw VaultError.failed("Couldn't unlock your messages.")
        }
        guard plain.count == 32 else { throw VaultError.failed("The stored database key is damaged.") }
        return plain
    }

    private func deleteWrappingKey() {
        let query: [String: Any] = [
            kSecClass as String: kSecClassKey,
            kSecAttrApplicationTag as String: wrapTag,
            kSecUseDataProtectionKeychain as String: true,
        ]
        SecItemDelete(query as CFDictionary)
    }

    // MARK: Keychain items

    private func baseQuery(_ account: String, dataProtection: Bool) -> [String: Any] {
        var q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrSynchronizable as String: false,
        ]
        if dataProtection { q[kSecUseDataProtectionKeychain as String] = true }
        return q
    }

    private func readItem(account: String) -> Data? {
        for dp in [true, false] {
            var q = baseQuery(account, dataProtection: dp)
            q[kSecReturnData as String] = true
            q[kSecMatchLimit as String] = kSecMatchLimitOne
            var out: CFTypeRef?
            if SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let data = out as? Data {
                return data
            }
        }
        return nil
    }

    private func writeItem(_ data: Data, account: String) throws {
        deleteItem(account: account)
        var q = baseQuery(account, dataProtection: true)
        q[kSecValueData as String] = data
        q[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        var status = SecItemAdd(q as CFDictionary, nil)
        if status == errSecMissingEntitlement {
            // Unsigned or ad-hoc builds can't use the data protection keychain.
            var legacy = baseQuery(account, dataProtection: false)
            legacy[kSecValueData as String] = data
            status = SecItemAdd(legacy as CFDictionary, nil)
        }
        guard status == errSecSuccess else { throw VaultError.failed("Couldn't save to the Keychain (\(status)).") }
    }

    private func deleteItem(account: String) {
        SecItemDelete(baseQuery(account, dataProtection: true) as CFDictionary)
        SecItemDelete(baseQuery(account, dataProtection: false) as CFDictionary)
    }
}

/// An optional app PIN with a duress PIN (architecture §13.3, Security). Entering the duress PIN
/// at the lock screen silently wipes Asiri. Only salted hashes are stored, in the Keychain.
public final class PINStore: @unchecked Sendable {
    public enum Outcome: Sendable, Equatable { case unlock, duress, wrong }

    private let service: String

    public init(service: String = "com.collinsadi.asiri.pin") {
        self.service = service
    }

    public var hasPIN: Bool { read("pin") != nil }
    public var hasDuressPIN: Bool { read("duress") != nil }

    public func setPIN(_ pin: String?) { save(pin, as: "pin") }
    public func setDuressPIN(_ pin: String?) { save(pin, as: "duress") }

    public func check(_ pin: String) -> Outcome {
        if let stored = read("duress"), matches(pin, stored) { return .duress }
        if let stored = read("pin"), matches(pin, stored) { return .unlock }
        return .wrong
    }

    public func clear() {
        save(nil, as: "pin")
        save(nil, as: "duress")
    }

    private func hash(_ pin: String, salt: Data) -> Data {
        // A deliberately slow-ish hash: many rounds of HMAC-SHA256 keyed by the salt.
        var value = Data(pin.utf8)
        let key = SymmetricKey(data: salt)
        for _ in 0 ..< 20_000 {
            value = Data(HMAC<SHA256>.authenticationCode(for: value, using: key))
        }
        return value
    }

    private func matches(_ pin: String, _ stored: Data) -> Bool {
        guard stored.count == 48 else { return false }
        let salt = stored.prefix(16)
        let expected = stored.suffix(32)
        let actual = hash(pin, salt: Data(salt))
        // constant-time comparison
        var diff: UInt8 = 0
        for (a, b) in zip(actual, expected) { diff |= a ^ b }
        return diff == 0
    }

    private func save(_ pin: String?, as account: String) {
        let query: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service, kSecAttrAccount as String: account]
        SecItemDelete(query as CFDictionary)
        guard let pin, !pin.isEmpty else { return }
        var salt = Data(count: 16)
        _ = salt.withUnsafeMutableBytes { SecRandomCopyBytes(kSecRandomDefault, 16, $0.baseAddress!) }
        var add = query
        add[kSecValueData as String] = salt + hash(pin, salt: salt)
        SecItemAdd(add as CFDictionary, nil)
    }

    private func read(_ account: String) -> Data? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service, kSecAttrAccount as String: account,
            kSecReturnData as String: true, kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var out: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &out) == errSecSuccess else { return nil }
        return out as? Data
    }
}
