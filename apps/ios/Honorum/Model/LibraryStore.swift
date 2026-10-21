import Foundation
import os

/// What this device knows about the account's documents: the revision it last
/// saw, the body it last agreed with, and anything still waiting to upload.
struct SyncState: Codable, Sendable {
    struct Known: Codable, Sendable, Hashable {
        var rev: Int64
        var type: String
        var deleted: Bool
        /// The canonical serialization the two sides agreed on, which is the
        /// base of the next three-way merge.
        var body: String
    }

    var seq: Int64 = 0
    var known: [String: Known] = [:]
    var conflicts: [ConflictCopy] = []
    var watchesVersion = ""
    /// True until the first download after signing in has been applied, which
    /// is the only moment the account may move this device to another term.
    var needsFirstPull = true
}

/// A remote version this device could not merge, kept so the edit it held is
/// recoverable from Settings rather than lost.
struct ConflictCopy: Codable, Sendable, Hashable, Identifiable {
    var id: String
    var document: String
    var kind: String
    var rev: Int64
    var at: Date
    var body: JSON

    var label: String {
        switch kind {
        case "schedule": body.text("name").isEmpty ? "A schedule" : body.text("name")
        case "task": body.text("title").isEmpty ? "A task" : body.text("title")
        case "prefs": "Preferences"
        default: document
        }
    }
}

/// One owner's saved state: the plan and the sync bookkeeping that goes with it.
struct Envelope: Codable, Sendable {
    var library = Library()
    var sync = SyncState()
}

/// The device's local library. Each owner — the guest, and every account that
/// has signed in here — has its own namespace, written atomically off the main
/// thread, so a save can never leave a half-written plan behind and signing in
/// or out never mixes one owner's plan into another's.
actor LibraryStore {
    static let guestOwner = "guest"

    private let directory: URL
    private let encoder = JSONEncoder()
    private let decoder = JSONDecoder()
    private static let log = Logger(subsystem: "app.honorum.ios", category: "library")

    init(directoryName: String = "Honorum") {
        directory = URL.applicationSupportDirectory.appending(
            path: directoryName, directoryHint: .isDirectory)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    private func url(_ owner: String) -> URL {
        let safe = owner.filter { $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" }
        return directory.appending(path: "library-\(safe.isEmpty ? Self.guestOwner : safe).json")
    }

    func load(owner: String) -> Envelope {
        guard let data = try? Data(contentsOf: url(owner)) else { return Envelope() }
        do {
            return try decoder.decode(Envelope.self, from: data)
        } catch {
            Self.log.error("Local library could not be read: \(error.localizedDescription)")
            return Envelope()
        }
    }

    func save(owner: String, _ envelope: Envelope) {
        do {
            let data = try encoder.encode(envelope)
            try data.write(to: url(owner), options: [.atomic, .completeFileProtection])
        } catch {
            Self.log.error("Local library could not be saved: \(error.localizedDescription)")
        }
    }

    /// Deleting an account removes what it stored here. The guest plan, which
    /// this device owns on its own, is never touched by that.
    func remove(owner: String) {
        try? FileManager.default.removeItem(at: url(owner))
    }
}
