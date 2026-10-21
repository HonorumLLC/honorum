import Foundation

/// Seat alerts. A watched section is part of the account's preferences, so the
/// watchlist follows the student to every signed-in device; the checking runs
/// on the device, against `GET /api/seats`, and announces an opening with a
/// local notification. Alerts created on the web are held by the server, which
/// delivers them through APNs, and stay visible and removable here.
extension AppState {
    var watches: [SeatWatch] { library.watching }

    func isWatching(_ sectionID: String) -> Bool {
        library.watching.contains { $0.section == sectionID }
    }

    /// Watches a section. Returns false when iOS has refused notifications:
    /// the watch is kept, since the app still checks it, but the student
    /// should know no alert will arrive.
    @discardableResult
    func watch(section: CourseSection, in course: Course, seats: Seats?) async -> Bool {
        guard let inst = library.inst, let term = library.term else { return true }
        var entry = SeatWatch(
            inst: inst, term: term, section: section.id,
            label: "\(course.code)-\(section.num)", title: course.title)
        entry.lastAvailable = seats?.available ?? -1
        guard !library.watching.contains(where: { $0.id == entry.id }) else { return true }
        library.watching.append(entry)
        scheduleSave()
        return await Reminders.shared.requestPermission()
    }

    func unwatch(sectionID: String) {
        library.watching.removeAll { $0.section == sectionID }
        scheduleSave()
    }

    /// Reads current counts for every watched section and announces the ones
    /// that have just opened. Runs at launch, when the app comes forward, and
    /// from a background refresh.
    func checkWatchedSeats() async {
        let watching = library.watching
        guard !watching.isEmpty else { return }
        var byTerm: [String: [SeatWatch]] = [:]
        for entry in watching { byTerm["\(entry.inst)/\(entry.term)", default: []].append(entry) }

        for (key, entries) in byTerm {
            let parts = key.split(separator: "/", maxSplits: 1).map(String.init)
            guard parts.count == 2 else { continue }
            guard
                let counts = try? await api.liveSeats(
                    inst: parts[0], term: parts[1], ids: entries.map(\.section))
            else { continue }

            for entry in entries {
                guard let seats = counts[entry.section], seats.isKnown else { continue }
                let wasOpen = entry.lastAvailable > 0
                if seats.isOpen && !wasOpen {
                    await Reminders.shared.announceSeat(entry, available: seats.available)
                }
                if let index = library.watching.firstIndex(where: { $0.id == entry.id }) {
                    library.watching[index].lastAvailable = seats.available
                }
            }
            await catalog.merge(live: counts)
        }
        scheduleSave()
    }

    /// Alerts the account holds, including ones created on the web.
    func refreshServerWatches() async {
        guard let token else { return }
        guard let found = try? await api.watches(token: token) else { return }
        setServerWatches(found)
    }

    func removeServerWatch(id: String) async {
        guard let token else { return }
        do {
            try await api.removeWatch(id: id, token: token)
        } catch {
            report("Could Not Remove Alert", error)
        }
        await refreshServerWatches()
    }
}
