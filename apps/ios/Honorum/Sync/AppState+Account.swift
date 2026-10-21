import Foundation
import UIKit

/// Signing in, signing out, and the document exchange that keeps this device's
/// plan and the account's in step.
extension AppState {
    var deviceName: String { UIDevice.current.name }

    // MARK: Signing in

    /// Asks the server to text a six-digit code. HTTP 202 means the provider
    /// accepted the request, never that a handset received it.
    func requestSignInCode(phone: String) async throws -> Int {
        let reply = try await api.requestCode(phone: phone)
        return reply.retryAfter ?? 30
    }

    /// Finishes sign-in. It returns once local ownership is on disk, so an
    /// interrupted first sync still leaves the device signed in and the sheet
    /// does not wait on the network; the first exchange catches up behind it.
    func verifySignIn(phone: String, code: String) async throws {
        let verified = try await api.verify(phone: phone, code: code, device: deviceName)
        let signedIn = Account(
            id: verified.id, phone: verified.phone, device: verified.device,
            seq: verified.seq, devices: verified.devices)

        // Keep the guest plan where it is; the account gets its own namespace.
        let guest = Envelope(library: library, sync: syncState)
        await store.save(owner: LibraryStore.guestOwner, guest)

        Keychain.write(.sessionToken, verified.token)
        Keychain.write(.accountID, verified.id)
        UserDefaults.standard.set(verified.id, forKey: "activeOwner")

        var envelope = await store.load(owner: verified.id)
        // An account that has never planned adopts what this device already has.
        if envelope.library.schedules.isEmpty && envelope.library.tasks.isEmpty
            && envelope.sync.known.isEmpty
        {
            envelope.library = guest.library
        }
        library = envelope.library
        syncState = envelope.sync
        setAccount(signedIn, token: verified.token)
        await saveAndWait()
        Task {
            await syncNow()
            startLiveUpdates()
            requestRemoteNotifications()
            await sendPushRegistration()
            await refreshAccount()
            await recordForegroundActivity()
            await refreshAssignments()
        }
    }

    // MARK: Live updates

    /// The server announces the account's document sequence after every
    /// commit. A moved sequence triggers the ordinary exchange; the stream is
    /// only a wakeup, so a missed frame costs nothing but a minute's delay.
    func startLiveUpdates() {
        guard liveTask == nil, let session = token, isSignedIn else { return }
        let accountID = owner
        liveGeneration += 1
        let generation = liveGeneration
        liveTask = Task { [weak self] in
            defer { if let self, self.liveGeneration == generation { self.liveTask = nil } }
            guard let api = self?.api else { return }
            do {
                let bytes = try await api.liveEvents(account: accountID, token: session)
                for try await line in bytes.lines {
                    guard let self, !Task.isCancelled, self.owner == accountID, self.token == session else { return }
                    guard line.hasPrefix("data: "), let event = JSON.decode(Data(line.dropFirst(6).utf8)) else { continue }
                    if Int64(event["seq"]?.numberValue ?? 0) > self.syncState.seq { await self.syncNow() }
                }
            } catch {
                // The next foreground turn or minute timer reconnects.
            }
        }
    }

    func stopLiveUpdates() {
        liveTask?.cancel()
        liveTask = nil
    }

    // MARK: The signed-in account

    func refreshAccount() async {
        guard let token else { return }
        do {
            let fresh = try await api.me(token: token)
            guard self.token == token else { return }
            setAccount(fresh, token: token)
        } catch HonorumAPI.Failure.unauthorized {
            guard self.token == token else { return }
            await forgetSession()
        } catch {}
    }

    func rename(device name: String) async {
        guard let token else { return }
        do {
            try await api.rename(device: name, token: token)
        } catch {
            report("Could Not Rename Device", error)
        }
        await refreshAccount()
    }

    func revoke(device id: String) async {
        guard let token else { return }
        do {
            try await api.revoke(device: id, token: token)
            if account?.device == id { await forgetSession() } else { await refreshAccount() }
        } catch {
            report("Could Not Remove Device", error)
        }
    }

    /// Signing out leaves an empty guest planner. Pending edits stay in the
    /// account's local namespace and come back when it signs in again.
    func signOut() async {
        if let token { try? await api.signOut(token: token) }
        saveNow()
        await forgetSession()
    }

    /// Removes the account's server-side data. Local plans stay on the device
    /// as a guest plan the student can upload again later.
    func deleteAccount() async throws {
        guard let token else { return }
        try await api.deleteAccount(token: token)
        let id = owner
        UserDefaults.standard.removeObject(forKey: "activity:" + id)
        UserDefaults.standard.removeObject(forKey: "activity-zone:" + id)
        UserDefaults.standard.removeObject(forKey: "canvas-connection:" + id)
        Self.forgetStreak(for: id)
        await store.remove(owner: id)
        await forgetSession()
    }

    private func forgetSession() async {
        stopLiveUpdates()
        await Task.detached { Keychain.removeSession() }.value
        UserDefaults.standard.set(LibraryStore.guestOwner, forKey: "activeOwner")
        setAccount(nil, token: nil)
        setServerWatches([])
        setSyncStatus(.idle)
        let envelope = await store.load(owner: LibraryStore.guestOwner)
        library = envelope.library
        syncState = envelope.sync
        ensureSchedule()
    }

    // MARK: Document exchange

    /// One catch-up with the account: upload what changed here, apply what
    /// changed elsewhere, and merge anything both sides touched. Follows the
    /// server's `more` flag and its own outbox until both are empty.
    func syncNow() async {
        if let job = accountSyncJob { await job.value; return }
        let job = Task { await performAccountSync() }
        accountSyncJob = job
        await job.value
        accountSyncJob = nil
    }

    private func performAccountSync() async {
        guard let token, isSignedIn else { return }
        let accountID = owner
        setSyncStatus(.syncing)
        var rounds = 0
        var changed = false
        do {
            while rounds < 20 {
                rounds += 1
                let documents = Sync.documents(of: library)
                let pending = Sync.pending(
                    documents, known: syncState.known, updated: Int64(Date().timeIntervalSince1970))
                // The first exchange after signing in only downloads, so the
                // account's plan is known before this device adds to it.
                let batch = syncState.needsFirstPull ? [] : Sync.batch(pending)
                let reply = try await api.sync(
                    docs: batch, since: syncState.seq,
                    watchesVersion: syncState.watchesVersion, token: token)

                guard owner == accountID && self.token == token else { return }
                // An empty poll agrees with what is already saved, so there is
                // nothing to apply or write; only an edit made meanwhile matters.
                let remaining: Bool
                if batch.isEmpty && isEmptyPoll(reply) {
                    remaining = !Sync.pending(
                        Sync.documents(of: library), known: syncState.known,
                        updated: Int64(Date().timeIntervalSince1970)).isEmpty
                } else {
                    changed = true
                    remaining = apply(reply, sent: batch)
                }
                if !reply.more && !remaining { break }
            }
            if rounds == 20 { setSyncStatus(.failed("More changes are waiting to sync. Try again.")) }
            else { setSyncStatus(.synced, at: Date()); startLiveUpdates() }
        } catch HonorumAPI.Failure.unauthorized {
            guard self.token == token else { return }
            await forgetSession()
            setSyncStatus(.failed("This device is signed out. Sign in again to sync."))
        } catch {
            guard self.token == token else { return }
            setSyncStatus(
                .failed((error as? LocalizedError)?.errorDescription ?? "Sync could not finish."))
        }
        if changed { saveNow() }
        // Task documents moved, so the streak the server counts from them may have.
        if streakStale { streakStale = false; await refreshStreak() }
    }

    private func isEmptyPoll(_ reply: SyncReply) -> Bool {
        reply.docs.isEmpty && reply.applied.isEmpty && reply.conflicts.isEmpty && !reply.more
            && reply.watches == nil && reply.seq == syncState.seq
            && (reply.watchesVersion ?? syncState.watchesVersion) == syncState.watchesVersion
            && !syncState.needsFirstPull
    }

    /// Applies one exchange and reports whether anything is still waiting to
    /// upload, so the caller does not canonicalize every document a third time.
    @discardableResult
    func apply(_ reply: SyncReply, sent: [SyncDoc]) -> Bool {
        let byID = Dictionary(uniqueKeysWithValues: sent.map { ($0.id, $0) })
        let latest = Dictionary(uniqueKeysWithValues: Sync.pending(
            Sync.documents(of: library), known: syncState.known,
            updated: Int64(Date().timeIntervalSince1970)).map { ($0.id, $0) })

        // Acknowledge only the exact versions this request uploaded.
        for accepted in reply.applied {
            guard let document = byID[accepted.id] else { continue }
            if document.deleted == true && accepted.rev == 0 {
                syncState.known[accepted.id] = nil
            } else {
                syncState.known[accepted.id] = Sync.remember(document, rev: accepted.rev)
            }
        }

        // A stale write comes back as a conflict carrying the current version.
        for remote in reply.conflicts {
            guard let mine = latest[remote.id] ?? byID[remote.id] else { continue }
            if Sync.isDeletionConflict(remote) {
                // The account deleted it; an offline device cannot restore it.
                syncState.known[remote.id] = Sync.remember(remote, rev: remote.rev ?? 0)
                Sync.apply(remote, to: &library, restoringNavigation: false)
                continue
            }
            if mine.deleted == true { syncState.known[remote.id] = Sync.remember(remote, rev: remote.rev ?? 0); continue }
            let resolved = Sync.resolve(local: mine, remote: remote, base: syncState.known[remote.id])
            if let copy = resolved.conflict { syncState.conflicts.append(copy) }
            syncState.known[remote.id] = Sync.remember(remote, rev: remote.rev ?? 0)
            Sync.apply(
                SyncDoc(id: remote.id, type: remote.type, body: resolved.merged), to: &library,
                restoringNavigation: false)
        }

        // Remote changes this device has not seen. On the first download the
        // account's navigation is restored and its version wins over whatever
        // this device adopted from the guest plan.
        let restoring = syncState.needsFirstPull
        let pending = Dictionary(uniqueKeysWithValues: Sync.pending(
            Sync.documents(of: library), known: syncState.known,
            updated: Int64(Date().timeIntervalSince1970)).map { ($0.id, $0) })
        for document in reply.docs {
            // A conflict may also be present in the downloaded page.
            if syncState.known[document.id]?.rev == document.rev { continue }
            if let mine = pending[document.id], document.deleted != true {
                if mine.deleted != true {
                    let base = Sync.base(for: mine, known: syncState.known[document.id], adopting: restoring)
                    let merged = Sync.resolve(local: mine, remote: document, base: base)
                    if let copy = merged.conflict { syncState.conflicts.append(copy) }
                    Sync.apply(SyncDoc(id: document.id, type: document.type, body: merged.merged), to: &library, restoringNavigation: restoring)
                }
            } else {
                Sync.apply(document, to: &library, restoringNavigation: restoring)
            }
            syncState.known[document.id] = Sync.remember(document, rev: document.rev ?? 0)
        }
        if restoring && !reply.more {
            syncState.needsFirstPull = false
            Sync.dropRedundantSchedules(from: &library, known: syncState.known)
            // An account that has planned before has no use for the setup pages.
            if library.holdsPlans { library.onboarded = true }
        }

        if (reply.docs + reply.conflicts).contains(where: { $0.id.hasPrefix("t_") })
            || reply.applied.contains(where: { $0.id.hasPrefix("t_") }) {
            streakStale = true
        }
        syncState.seq = reply.seq
        if let version = reply.watchesVersion { syncState.watchesVersion = version }
        if let watches = reply.watches { setServerWatches(watches) }
        ensureSchedule()
        return !Sync.pending(Sync.documents(of: library), known: syncState.known, updated: Int64(Date().timeIntervalSince1970)).isEmpty
    }

    func restoreConflict(id: String) {
        guard let copy = syncState.conflicts.first(where: { $0.id == id }),
              Sync.recover(copy, in: &library) else { return }
        syncState.conflicts.removeAll { $0.id == id }
        saveNow()
    }

    /// Drops an archived remote copy once the student has looked at it.
    func discardConflict(id: String) {
        syncState.conflicts.removeAll { $0.id == id }
        saveNow()
    }
}


extension AppState {
    /// A foreground check-in reads the streak. The server counts days that
    /// ended with nothing overdue from the account's tasks, so the visit
    /// itself counts for nothing; the first one fixes the account's time zone
    /// from its school.
    func recordForegroundActivity() async {
        guard UIApplication.shared.applicationState == .active, let session = token else { return }
        let accountID = owner
        guard !activitySending else { return }
        activitySending = true
        defer { activitySending = false }
        do {
            let result = try await api.recordActivity(inst: library.inst ?? "", token: session)
            guard owner == accountID && token == session else { return }
            streak = result
            cache(streak: result, for: accountID)
            // Visits queued by earlier versions no longer count.
            UserDefaults.standard.removeObject(forKey: "activity:" + accountID)
            UserDefaults.standard.removeObject(forKey: "activity-zone:" + accountID)
        } catch { }
    }

    /// The streak as the server counts it now, after this device's task
    /// documents reached it or another device's arrived.
    func refreshStreak() async {
        guard let session = token else { return }
        let accountID = owner
        do {
            let result = try await api.accountStreak(token: session)
            guard owner == accountID && token == session else { return }
            streak = result
            cache(streak: result, for: accountID)
        } catch { }
    }
}
