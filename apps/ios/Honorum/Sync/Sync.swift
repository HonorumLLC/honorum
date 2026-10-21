import Foundation

/// The document layer of account sync. These are pure functions over a library
/// and what this device last agreed with, so the exchange itself stays a short
/// loop and every rule here is testable on its own.
///
/// Schedules are `s_<id>`, tasks are `t_<id>`, and preferences are `prefs`,
/// the same identities and bodies the web client reads and writes.
enum Sync {
    static let maxDocumentsPerRequest = 200
    static let maxUploadBytes = (4 << 20) - 2048
    static let maxBodyBytes = 64 << 10

    struct Document: Sendable {
        var type: String
        var body: JSON
    }

    /// Every document the saved library currently represents.
    static func documents(of library: Library) -> [String: Document] {
        var out: [String: Document] = [:]
        for schedule in library.schedules {
            out["s_\(schedule.id)"] = Document(type: "schedule", body: schedule.body)
        }
        for task in library.tasks {
            out["t_\(task.id)"] = Document(type: "task", body: task.body)
        }
        out["prefs"] = Document(type: "prefs", body: library.prefsBody)
        return out
    }

    static func recover(_ copy: ConflictCopy, in library: inout Library) -> Bool {
        guard case .object(let fields) = copy.body, !fields.isEmpty else { return false }
        switch copy.kind {
        case "schedule":
            var schedule = Schedule(copy.body)
            schedule.id = UUID().uuidString.lowercased()
            schedule.name += " (recovered)"
            schedule.primary = false
            library.schedules.append(schedule)
        case "task":
            var task = StudentTask(copy.body)
            task.id = UUID().uuidString.lowercased()
            task.source = .manual
            task.uid = ""
            task.pendingDone = nil
            task.pendingCanvasDone = nil
            task.extras.removeValue(forKey: "canvasConnection")
            library.tasks.append(task)
        default: return false
        }
        return true
    }

    /// What has changed since the server last agreed: edited documents, plus a
    /// deletion for anything the account still holds that the library dropped.
    static func pending(
        _ documents: [String: Document], known: [String: SyncState.Known], updated: Int64
    )
        -> [SyncDoc]
    {
        var out: [SyncDoc] = []
        for (id, document) in documents {
            let canonical = document.body.canonical
            let seen = known[id]
            guard seen == nil || seen!.deleted || seen!.body != canonical else { continue }
            out.append(
                SyncDoc(
                    id: id, type: document.type, base: seen?.rev ?? 0, updated: updated,
                    body: document.body))
        }
        for (id, seen) in known where documents[id] == nil && !seen.deleted {
            out.append(
                SyncDoc(id: id, type: seen.type, base: seen.rev, updated: updated, deleted: true))
        }
        // A stable order keeps a large first upload deterministic across runs.
        return out.sorted { $0.id < $1.id }
    }

    /// As many pending documents as one request may carry.
    static func batch(_ pending: [SyncDoc]) -> [SyncDoc] {
        var out: [SyncDoc] = []
        var size = 0
        for document in pending {
            let bytes = (document.body?.data.count ?? 0) + document.id.utf8.count + 64
            if out.count == maxDocumentsPerRequest || size + bytes > maxUploadBytes {
                if out.isEmpty { out.append(document) }  // one oversized document still tries
                break
            }
            out.append(document)
            size += bytes
        }
        return out
    }

    /// Remembers the version the two sides now agree on.
    static func remember(_ document: SyncDoc, rev: Int64) -> SyncState.Known {
        SyncState.Known(
            rev: rev, type: document.type, deleted: document.deleted == true,
            body: document.deleted == true ? "" : (document.body?.canonical ?? ""))
    }

    /// The common ancestor for merging a document the account has never
    /// acknowledged. During the first download after signing in, the copy here
    /// is what the device adopted from its guest plan: defaults nobody chose,
    /// like the first school listed and an unfinished tour. It stands as the
    /// base, so the account's version wins wherever the two differ, the way the
    /// web adopts an account. Afterwards such a document is a local edit with
    /// no ancestor, and the usual rule keeps it.
    static func base(for local: SyncDoc, known: SyncState.Known?, adopting: Bool)
        -> SyncState.Known?
    {
        known ?? (adopting ? remember(local, rev: 0) : nil)
    }

    /// Once the account's plans have arrived, an empty schedule this device
    /// made for a term the account already plans is clutter rather than work:
    /// uploading it would add a second primary schedule to that term.
    static func dropRedundantSchedules(
        from library: inout Library, known: [String: SyncState.Known]
    ) {
        let fromAccount = { (schedule: Schedule) in known["s_\(schedule.id)"] != nil }
        let planned = Set(library.schedules.filter(fromAccount).map { "\($0.inst)/\($0.term)" })
        library.schedules.removeAll { schedule in
            !fromAccount(schedule) && schedule.sections.isEmpty && schedule.events.isEmpty
                && planned.contains("\(schedule.inst)/\(schedule.term)")
        }
    }

    /// Applies a downloaded document. Navigation — the school, term, and open
    /// schedule — is restored only on the first download after signing in, so a
    /// later background poll never moves the student off the week they are on.
    static func apply(_ document: SyncDoc, to library: inout Library, restoringNavigation: Bool) {
        if document.id == "prefs" {
            guard document.deleted != true, let body = document.body else { return }
            library.apply(prefs: body, restoringNavigation: restoringNavigation)
            return
        }
        let id = String(document.id.dropFirst(2))
        if document.id.hasPrefix("s_") {
            if document.deleted == true {
                library.schedules.removeAll { $0.id == id }
            } else if let body = document.body {
                let schedule = Schedule(body)
                if let index = library.schedules.firstIndex(where: { $0.id == id }) {
                    library.schedules[index] = schedule
                } else {
                    library.schedules.append(schedule)
                }
            }
        } else if document.id.hasPrefix("t_") {
            if document.deleted == true {
                library.tasks.removeAll { $0.id == id }
            } else if let body = document.body {
                let task = StudentTask(body)
                if let index = library.tasks.firstIndex(where: { $0.id == id }) {
                    library.tasks[index] = task
                } else {
                    library.tasks.append(task)
                }
            }
        }
    }

    /// Combines this device's edit with the version the server already had.
    /// Independent fields both survive; when the two sides changed the same
    /// field the local edit stands and the remote copy is archived.
    static func resolve(local: SyncDoc, remote: SyncDoc, base: SyncState.Known?)
        -> (merged: JSON, conflict: ConflictCopy?)
    {
        let baseBody = base.flatMap { JSON.decode(Data($0.body.utf8)) } ?? .object([:])
        let localBody = local.body ?? .object([:])
        let remoteBody = remote.body ?? .object([:])
        let result = JSON.merge(base: baseBody, local: localBody, remote: remoteBody)
        guard result.conflicted else { return (result.body, nil) }
        let copy = ConflictCopy(
            id: "\(remote.id)@\(remote.rev ?? 0)", document: remote.id, kind: remote.type,
            rev: remote.rev ?? 0, at: Date(), body: remoteBody)
        return (result.body, copy)
    }

    /// A schedule or task the server deleted while this device was editing it
    /// cannot come back: the deletion record outlives the offline copy.
    static func isDeletionConflict(_ remote: SyncDoc) -> Bool { remote.deleted == true }
}
