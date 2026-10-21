import Foundation

/// The account this device is signed into.
struct Account: Codable, Sendable, Hashable {
    var id: String
    var phone: String
    var device: String
    var seq: Int64?
    var devices: [Device] = []

    struct Device: Codable, Sendable, Hashable, Identifiable {
        var id: String
        var name: String
        var created: Double
        var lastSeen: Double
        var push: Bool
        var current: Bool

        var lastSeenDate: Date { Date(timeIntervalSince1970: lastSeen) }
        var createdDate: Date { Date(timeIntervalSince1970: created) }
    }
}

/// One account document as it crosses the wire.
struct SyncDoc: Codable, Sendable, Hashable {
    var id: String
    var type: String
    var rev: Int64?
    var base: Int64?
    var updated: Int64?
    var deleted: Bool?
    var body: JSON?
}

struct SyncReply: Codable, Sendable {
    var seq: Int64
    var docs: [SyncDoc] = []
    var more: Bool = false
    var applied: [Applied] = []
    var conflicts: [SyncDoc] = []
    /// Watch changes carry their own version, independent of document revisions.
    var watchesVersion: String?
    var watches: [ServerWatch]?

    struct Applied: Codable, Sendable, Hashable {
        var id: String
        var rev: Int64
    }
}

extension HonorumAPI {
    /// The SMS notice version this client shows. It has to match the server's
    /// `smsConsentVersion`, which records the consent before any text is sent.
    static let smsConsentVersion = "2026-09-08-v2"

    // MARK: Sign-in

    struct CodeSent: Codable, Sendable {
        var retryAfter: Int?
    }

    func requestCode(phone: String) async throws -> CodeSent {
        try await send(
            "POST", "api/auth/code",
            body: ["phone": .of(phone), "consent": .of(Self.smsConsentVersion)])
    }

    struct Verified: Codable, Sendable {
        var id: String
        var phone: String
        var device: String
        var seq: Int64?
        var devices: [Account.Device] = []
        var token: String
    }

    func verify(phone: String, code: String, device: String) async throws -> Verified {
        try await send(
            "POST", "api/auth/verify",
            body: ["phone": .of(phone), "code": .of(code), "device": .of(device)])
    }

    // MARK: The signed-in account

    func me(token: String) async throws -> Account {
        try await send("GET", "api/me", token: token)
    }

    func rename(device name: String, token: String) async throws {
        try await sendVoid("PATCH", "api/me", body: ["device": .of(name)], token: token)
    }

    func revoke(device id: String, token: String) async throws {
        try await sendVoid("DELETE", "api/me/devices/\(id)", token: token)
    }

    func signOut(token: String) async throws {
        try await sendVoid("POST", "api/auth/logout", token: token)
    }

    func deleteAccount(token: String) async throws {
        try await sendVoid("DELETE", "api/me", token: token)
    }

    // MARK: Documents

    func sync(docs: [SyncDoc], since: Int64, watchesVersion: String, token: String) async throws
        -> SyncReply
    {
        var body: [String: JSON] = [
            "since": .of(Int(since)),
            "docs": .array(docs.map { document in
                var fields: [String: JSON] = [
                    "id": .of(document.id), "type": .of(document.type),
                    "base": .of(Int(document.base ?? 0)),
                    "updated": .of(Int(document.updated ?? 0)),
                ]
                if document.deleted == true {
                    fields["deleted"] = .of(true)
                } else {
                    fields["body"] = document.body ?? .object([:])
                }
                return .object(fields)
            }),
        ]
        if !watchesVersion.isEmpty { body["watches_version"] = .of(watchesVersion) }
        return try await send("POST", "api/me/sync", body: body, token: token)
    }

    // MARK: Seat alerts

    func watches(token: String) async throws -> [ServerWatch] {
        try await send("GET", "api/me/watches", token: token)
    }

    func removeWatch(id: String, token: String) async throws {
        try await sendVoid("DELETE", "api/watches/\(id)", token: token)
    }

    // MARK: Assignments

    /// Relays a guest Canvas import. Credentials pass through the server and are
    /// never stored there; they stay in this device's Keychain.
    func canvasTasks(base: String, token: String) async throws -> [ImportedTask] {
        try await send(
            "POST", "api/lms/canvas", body: ["base": .of(base), "token": .of(token)], timeout: 40)
    }

    /// A guest's Canvas courses matched to the school's catalog. The token is
    /// relayed and never kept.
    func canvasClasses(base: String, token: String, inst: String, term: String) async throws -> FoundClasses {
        try await send(
            "POST", "api/lms/canvas/classes",
            body: ["base": .of(base), "token": .of(token), "inst": .of(inst), "term": .of(term)], timeout: 40)
    }

    func canvasComplete(
        base: String, token: String, type: String, id: String, overrideID: String,
        courseID: String, moduleItem: String, done: Bool
    ) async throws {
        try await sendVoid(
            "POST", "api/lms/canvas/complete",
            body: [
                "base": .of(base), "token": .of(token), "type": .of(type), "id": .of(id),
                "overrideId": .of(overrideID), "courseId": .of(courseID),
                "moduleItem": .of(moduleItem), "done": .of(done),
            ], timeout: 40)
    }

    /// Fetches an iCalendar feed through the relay, which checks the
    /// destination and bounds the response.
    func calendarFeed(url: String) async throws -> String {
        let data = try await raw("POST", "api/lms/feed", body: ["url": .of(url)], timeout: 40)
        return String(decoding: data, as: UTF8.self)
    }
}

/// A normalized assignment from the relay.
/// Canvas courses the server matched to the school's catalog: the sections it
/// settled on, the choices it left (like the lab a lecture needs), and every
/// mentioned section's catalog copy, so they save without the catalog.
struct FoundClasses: Codable, Sendable {
    struct Source: Codable, Sendable, Hashable {
        var id: String
        var name: String
        var code: String
    }
    struct Match: Codable, Sendable, Hashable, Identifiable {
        var course: Source
        var code: String
        var title: String
        var sections: [String]
        var options: [[String]]
        /// Canvas named the sections, rather than the instructor or a lone
        /// section implying them.
        var sure: Bool
        var id: String { course.id }
    }
    var term: String
    var classes: [Match]
    var unmatched: [Source]
    var sections: [String: JSON]
}

struct ImportedTask: Codable, Sendable, Hashable {
    var uid: String
    var title: String
    var course: String
    var courseId: String?
    var courseCode: String?
    var due: String
    var done: Bool
    var url: String?
    var overrideId: String?
    var moduleItem: String?
    var reason: String?
}

/// Shared account features. Credentials are never returned by these endpoints.
struct CanvasConnection: Codable, Sendable, Hashable {
    var id: String
    var base: String
    var connected: Bool
    var available: Bool
    var synced: Int64
    var error: String
}

/// The account's streak as the server counts it: consecutive days that ended
/// with nothing overdue. `days` lists up to 28 days ending today; `overdue`
/// counts tasks open past their due time now.
struct AccountStreak: Codable, Sendable, Hashable {
    struct Day: Codable, Sendable, Hashable {
        var day: String
        /// `clean` or `missed` for a finished day, `clean` or `overdue` for today.
        var state: String
    }

    var current: Int
    var longest: Int
    var today: String
    var lastDay: String
    var timezone: String
    var overdue: Int?
    var days: [Day]?
}

extension HonorumAPI {
    func accountCanvas(token: String) async throws -> CanvasConnection {
        try await send("GET", "api/me/canvas", token: token)
    }
    func connectAccountCanvas(base: String, credential: String, token: String) async throws -> CanvasConnection {
        try await send("PUT", "api/me/canvas", body: ["base": .of(base), "token": .of(credential)], token: token, timeout: 40)
    }
    func disconnectAccountCanvas(token: String) async throws -> CanvasConnection {
        try await send("DELETE", "api/me/canvas", token: token)
    }
    /// The account's Canvas courses matched to the catalog, with its saved token.
    func accountCanvasClasses(inst: String, term: String, token: String) async throws -> FoundClasses {
        try await send("POST", "api/me/canvas/classes", body: ["inst": .of(inst), "term": .of(term)], token: token, timeout: 40)
    }
    func refreshAccountCanvas(force: Bool, token: String) async throws -> CanvasConnection {
        try await send("POST", "api/me/canvas/refresh", body: ["force": .of(force)], token: token, timeout: 40)
    }
    struct CanvasCompletionReply: Codable, Sendable { var applied: Bool; var module: Bool? }
    func completeAccountCanvas(document: String, intent: String, token: String) async throws -> CanvasCompletionReply {
        try await send("POST", "api/me/canvas/complete", body: ["document": .of(document), "intent": .of(intent)], token: token, timeout: 40)
    }
    /// A foreground check-in. The first fixes the account's time zone from its
    /// school; visits themselves count for nothing. Returns the streak.
    func recordActivity(inst: String, token: String) async throws -> AccountStreak {
        try await send("POST", "api/me/activity", body: ["inst": .of(inst)], token: token)
    }
    func accountStreak(token: String) async throws -> AccountStreak {
        try await send("GET", "api/me/streak", token: token)
    }
}

/// A calendar feed the account holds. The address stays on the server.
struct AccountFeed: Codable, Sendable, Hashable, Identifiable {
    var id: String
    var label: String
    var tz: String?
    var synced: Int64
    var error: String
}

struct AccountFeeds: Codable, Sendable {
    var feeds: [AccountFeed] = []
    var available: Bool = false
}

/// An earlier version of one document, kept by the server for thirty days.
struct DocumentVersion: Codable, Sendable {
    var rev: Int64
    var updated: Int64
    var archived: Int64
    var deleted: Bool?
    var body: JSON?
}

struct DocumentHistory: Codable, Sendable {
    var id: String
    var versions: [DocumentVersion] = []
}

extension HonorumAPI {
    // MARK: Calendar feeds, imported by the server for every device

    func accountFeeds(token: String) async throws -> AccountFeeds {
        try await send("GET", "api/me/feeds", token: token)
    }
    func addAccountFeed(url: String, label: String, token: String) async throws -> AccountFeeds {
        try await send("PUT", "api/me/feeds", body: ["url": .of(url), "label": .of(label)], token: token, timeout: 40)
    }
    func removeAccountFeed(id: String, token: String) async throws -> AccountFeeds {
        try await send("DELETE", "api/me/feeds/\(id)", token: token)
    }
    func refreshAccountFeeds(force: Bool, token: String) async throws -> AccountFeeds {
        try await send("POST", "api/me/feeds/refresh", body: ["force": .of(force)], token: token, timeout: 40)
    }

    // MARK: Native push

    /// Gives the server this phone's APNs token, so account alerts and
    /// reminders arrive without the app running.
    func registerPush(apns deviceToken: String, session: String) async throws {
        try await sendVoid("PUT", "api/me/push", body: ["apns": .of(deviceToken)], token: session)
    }

    // MARK: History and live updates

    func history(document: String, token: String) async throws -> DocumentHistory {
        try await send("GET", "api/me/history/\(document)", token: token)
    }

    /// The account's document sequence, announced after every commit.
    func liveEvents(account: String, token: String) async throws -> URLSession.AsyncBytes {
        try await stream("api/me/events", query: ["account": account], token: token)
    }
}
