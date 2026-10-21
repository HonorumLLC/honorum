import Foundation
import Security

/// Device-only credentials. The session token, the guest Canvas address and personal
/// access token, and calendar feed URLs live here and never enter account sync
/// or the local library file.
enum Keychain {
    enum Item: String, CaseIterable {
        case sessionToken = "session-token"
        case accountID = "account-id"
        case canvasBase = "canvas-base"
        case canvasToken = "canvas-token"
        case feedURLs = "feed-urls"
    }

    static func read(_ item: Item) -> String? {
        var query = base(item)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
            let data = result as? Data
        else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func write(_ item: Item, _ value: String?) {
        guard let value, !value.isEmpty else { return remove(item) }
        let data = Data(value.utf8)
        let query = base(item)
        let attributes: [String: Any] = [kSecValueData as String: data]
        if SecItemUpdate(query as CFDictionary, attributes as CFDictionary) == errSecSuccess {
            return
        }
        var insert = query
        insert[kSecValueData as String] = data
        insert[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        SecItemAdd(insert as CFDictionary, nil)
    }

    static func remove(_ item: Item) {
        SecItemDelete(base(item) as CFDictionary)
    }

    /// Signing out clears every device credential the account granted, and
    /// leaves the ones the student typed on this device alone.
    static func removeSession() {
        remove(.sessionToken)
        remove(.accountID)
    }

    private static func base(_ item: Item) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "app.honorum.ios",
            kSecAttrAccount as String: item.rawValue,
        ]
    }
}

extension Keychain {
    /// Calendar feed URLs, stored as one newline-separated list.
    static var feeds: [String] {
        get { (read(.feedURLs) ?? "").split(separator: "\n").map(String.init) }
        set { write(.feedURLs, newValue.joined(separator: "\n")) }
    }
}
