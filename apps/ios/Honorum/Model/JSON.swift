import Foundation

/// A JSON value kept whole. Account documents are shared with the web client,
/// which stores fields this app has never heard of; carrying the raw value
/// means an edit here can never silently drop them.
@frozen
indirect enum JSON: Codable, Hashable, Sendable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSON])
    case object([String: JSON])

    init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSON].self) {
            self = .array(value)
        } else {
            self = .object(try container.decode([String: JSON].self))
        }
    }

    func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case .bool(let value): try container.encode(value)
        case .number(let value):
            // Whole numbers encode as integers, matching what the web writes.
            if value == value.rounded(), abs(value) < 9_007_199_254_740_992 {
                try container.encode(Int64(value))
            } else {
                try container.encode(value)
            }
        case .string(let value): try container.encode(value)
        case .array(let value): try container.encode(value)
        case .object(let value): try container.encode(value)
        }
    }

    // MARK: Reading

    var objectValue: [String: JSON]? { if case .object(let value) = self { value } else { nil } }
    var arrayValue: [JSON]? { if case .array(let value) = self { value } else { nil } }
    var stringValue: String? { if case .string(let value) = self { value } else { nil } }
    var numberValue: Double? { if case .number(let value) = self { value } else { nil } }
    var intValue: Int? { numberValue.map(Int.init) }
    var boolValue: Bool? {
        switch self {
        case .bool(let value): value
        case .number(let value): value != 0
        default: nil
        }
    }

    subscript(key: String) -> JSON? { objectValue?[key] }

    /// The value at `key` as text, treating a missing key and an empty string
    /// alike, the way the web client's `|| ''` defaults do.
    func text(_ key: String) -> String { self[key]?.stringValue ?? "" }
    func int(_ key: String) -> Int { self[key]?.intValue ?? 0 }
    func flag(_ key: String) -> Bool { self[key]?.boolValue ?? false }
    func list(_ key: String) -> [JSON] { self[key]?.arrayValue ?? [] }
    func map(_ key: String) -> [String: JSON] { self[key]?.objectValue ?? [:] }

    var isEmptyObject: Bool { objectValue?.isEmpty ?? false }

    // MARK: Writing

    static func object(_ pairs: KeyValuePairs<String, JSON>) -> JSON {
        .object(Dictionary(uniqueKeysWithValues: pairs.map { ($0.key, $0.value) }))
    }

    static func of(_ value: String) -> JSON { .string(value) }
    static func of(_ value: Int) -> JSON { .number(Double(value)) }
    static func of(_ value: Double) -> JSON { .number(value) }
    static func of(_ value: Bool) -> JSON { .bool(value) }

    /// Merges extra keys another client wrote back under the ones this client
    /// owns, so a round trip through the app preserves them.
    func merging(extras: [String: JSON]) -> JSON {
        guard case .object(let mine) = self else { return self }
        return .object(extras.merging(mine) { _, own in own })
    }

    /// Keys this client does not write, kept so they survive an edit here.
    func extras(besides known: Set<String>) -> [String: JSON] {
        (objectValue ?? [:]).filter { !known.contains($0.key) }
    }

    // MARK: Bytes

    static func decode(_ data: Data) -> JSON? {
        try? JSONDecoder().decode(JSON.self, from: data)
    }

    var data: Data {
        // Sorted keys give a document one canonical serialization, so the
        // outbox can compare a saved body against the one on the server.
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return (try? encoder.encode(self)) ?? Data("null".utf8)
    }

    var canonical: String { String(decoding: data, as: UTF8.self) }
}

// MARK: - Three-way merge

extension JSON {
    /// Combines two independent edits of the same document, matching the web
    /// client's merge so both clients resolve a divergence identically.
    ///
    /// Independent fields both survive. Additions to and removals from a list
    /// of ids both apply, and a list of objects with ids, such as personal
    /// events, merges item by item. When the two sides changed the same field,
    /// the local edit wins and the caller archives the remote copy for recovery.
    static func merge(base: JSON, local: JSON, remote: JSON) -> (body: JSON, conflicted: Bool) {
        var conflicted = false

        func combine(_ base: JSON?, _ local: JSON?, _ remote: JSON?) -> JSON? {
            if local == remote || remote == base { return local }
            if local == base { return remote }

            // A list of ids merges by set: what either side added stays, what
            // either side removed goes.
            if let base = base?.arrayValue, let local = local?.arrayValue,
                let remote = remote?.arrayValue,
                (base + local + remote).allSatisfy({ $0.stringValue != nil })
            {
                var seen = Set<JSON>()
                var out: [JSON] = []
                for value in local where !base.contains(value) || remote.contains(value) {
                    if seen.insert(value).inserted { out.append(value) }
                }
                for value in remote where !base.contains(value) {
                    if seen.insert(value).inserted { out.append(value) }
                }
                return .array(out)
            }

            // An item removed on one side goes, unless the other side edited it.
            if let base = base?.arrayValue, let local = local?.arrayValue,
                let remote = remote?.arrayValue,
                let baseItems = identified(base), let localItems = identified(local),
                let remoteItems = identified(remote)
            {
                var out: [JSON] = []
                for (id, item) in localItems.ordered {
                    if let other = remoteItems.byID[id] {
                        out.append(combine(baseItems.byID[id], item, other) ?? item)
                    } else if let original = baseItems.byID[id] {
                        if original != item {
                            conflicted = true
                            out.append(item)
                        }
                    } else {
                        out.append(item)
                    }
                }
                for (id, item) in remoteItems.ordered where localItems.byID[id] == nil {
                    if let original = baseItems.byID[id] {
                        if original != item { conflicted = true }
                    } else {
                        out.append(item)
                    }
                }
                return .array(out)
            }

            if let baseObject = base?.objectValue, let localObject = local?.objectValue,
                let remoteObject = remote?.objectValue
            {
                var keys: [String] = []
                var seen = Set<String>()
                for key in baseObject.keys.sorted() + localObject.keys.sorted()
                    + remoteObject.keys.sorted() where seen.insert(key).inserted
                {
                    keys.append(key)
                }
                var merged: [String: JSON] = [:]
                for key in keys {
                    if let value = combine(baseObject[key], localObject[key], remoteObject[key]) {
                        merged[key] = value
                    }
                }
                return .object(merged)
            }

            conflicted = true
            return local
        }

        return (combine(base, local, remote) ?? local, conflicted)
    }

    /// A list's objects keyed by their string ids, in order, or nil when any
    /// element lacks a unique one.
    private static func identified(_ list: [JSON]) -> (ordered: [(String, JSON)], byID: [String: JSON])? {
        var ordered: [(String, JSON)] = []
        var byID: [String: JSON] = [:]
        for value in list {
            guard let id = value.objectValue?["id"]?.stringValue, byID[id] == nil else { return nil }
            ordered.append((id, value))
            byID[id] = value
        }
        return (ordered, byID)
    }
}
