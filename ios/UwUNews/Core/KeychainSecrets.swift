import Foundation
import Security
import UwUNewsCore

/// Where the engine keeps its keys (the vault key, the sync device key):
/// the Keychain, readable after the first unlock so background refresh and
/// sync work with a locked phone, and never in a backup to another device.
final class KeychainSecrets: SecretStore, @unchecked Sendable {
    private let service = "app.uwunews.ios.secrets"

    func get(name: String) -> Data? {
        var query = base(name)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess else { return nil }
        return item as? Data
    }

    func set(name: String, value: Data) {
        let attributes: [String: Any] = [
            kSecValueData as String: value,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        if SecItemUpdate(base(name) as CFDictionary, attributes as CFDictionary) == errSecItemNotFound {
            var add = base(name)
            add.merge(attributes) { $1 }
            SecItemAdd(add as CFDictionary, nil)
        }
    }

    func delete(name: String) {
        SecItemDelete(base(name) as CFDictionary)
    }

    private func base(_ name: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: name,
        ]
    }
}
