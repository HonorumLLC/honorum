import Foundation

/// Progress rings: how much of today, this week and this term is done, the
/// same counting as the web's `rings.js`. Today and This Week follow
/// Coursicle's rule: an overdue task stays in them until it is done, and
/// finishing one counts in the period it was finished. This Term counts from
/// a record kept on the device, because imports drop tasks a week or two
/// after they are due.
enum RingPeriod: String, CaseIterable, Identifiable, Sendable {
    case today, week, term

    var id: String { rawValue }

    var label: String {
        switch self {
        case .today: "Today"
        case .week: "This Week"
        case .term: "This Term"
        }
    }

}

struct RingCount: Hashable, Sendable {
    var done = 0
    var total = 0

    /// A period with nothing in it is complete: there is nothing left to do.
    var percent: Int { total == 0 ? 100 : Int((Double(done) / Double(total) * 100).rounded()) }
}

struct RingCounts: Hashable, Sendable {
    var today = RingCount()
    var week = RingCount()
    var term = RingCount()

    subscript(period: RingPeriod) -> RingCount {
        switch period {
        case .today: today
        case .week: week
        case .term: term
        }
    }
}

enum Rings {
    /// Counts for the three rings on campus days. `only` limits them to one
    /// class the way the task filter does: empty for every class, a code for
    /// that class, or `__none` for tasks matched to no class. `codeOf` names a
    /// task's class the same way the record does.
    static func counts(
        tasks: [StudentTask], now: Date = Date(), zone: TimeZone, weekStart: Int = 6,
        record: TermRecord = TermRecord(), only: String = "", codeOf: (StudentTask) -> String
    ) -> RingCounts {
        let mine = only.isEmpty ? tasks : tasks.filter { matches(codeOf($0), only) }
        let today = CivilDate(now, in: zone)
        let week = today.startOfWeek(weekStart)
        let term = record.entries.values.filter { only.isEmpty || matches($0.code, only) }
        return RingCounts(
            today: count(mine.filter { inPeriod($0, from: today, to: today.adding(days: 1), zone: zone) }),
            week: count(mine.filter { inPeriod($0, from: week, to: week.adding(days: 7), zone: zone) }),
            term: RingCount(done: term.filter(\.done).count, total: term.count))
    }

    static func matches(_ code: String, _ only: String) -> Bool {
        only.isEmpty || (only == "__none" ? code.isEmpty : code == only)
    }

    private static func count(_ list: [StudentTask]) -> RingCount {
        RingCount(done: list.filter(\.done).count, total: list.count)
    }

    /// A task is in a period when it is due within it, or was due earlier and
    /// is still open or was finished during the period.
    private static func inPeriod(_ task: StudentTask, from: CivilDate, to: CivilDate, zone: TimeZone) -> Bool {
        guard let due = task.dueDate else { return false }
        let day = CivilDate(due, in: zone)
        if day >= to { return false }
        if day >= from { return true }
        if !task.done { return true }
        guard let finished = task.completedAt else { return false }
        return CivilDate(finished, in: zone) >= from
    }
}

/// One entry per task due in the term, kept on this device for each account,
/// school and term, so work stays counted after imports drop it. Tasks in the
/// list are authoritative; a task that left the list stays only when it was
/// due more than a week ago, since that is when imports stop carrying it. A
/// task the student deletes leaves however old it is.
struct TermRecord: Codable, Equatable, Sendable {
    struct Entry: Codable, Equatable, Sendable {
        var done: Bool
        var code: String
        /// The deadline, in milliseconds since 1970, as the web records it.
        var due: Int64
    }

    var entries: [String: Entry] = [:]

    static func key(owner: String, inst: String, term: String) -> String {
        "term-record:\(owner):\(inst):\(term)"
    }

    static func load(_ key: String, from defaults: UserDefaults = .standard) -> TermRecord {
        guard let data = defaults.data(forKey: key),
              let record = try? JSONDecoder().decode(TermRecord.self, from: data) else { return TermRecord() }
        return record
    }

    func save(_ key: String, to defaults: UserDefaults = .standard) {
        if let data = try? JSONEncoder().encode(self) { defaults.set(data, forKey: key) }
    }

    /// The record brought up to date with the current tasks.
    func updated(
        with tasks: [StudentTask], start: String?, end: String?, zone: TimeZone, now: Date = Date(),
        codeOf: (StudentTask) -> String
    ) -> TermRecord {
        let from = CivilDate(start ?? "")
        let to = CivilDate(end ?? "")?.adding(days: 1)
        var next: [String: Entry] = [:]
        var seen: Set<String> = []
        for task in tasks {
            let key = task.uid.isEmpty ? task.id : task.uid
            seen.insert(key)
            guard !key.isEmpty, let due = task.dueDate else { continue }
            let day = CivilDate(due, in: zone)
            if let from, day < from { continue }
            if let to, day >= to { continue }
            next[key] = Entry(done: task.done, code: codeOf(task), due: Int64(due.timeIntervalSince1970 * 1000))
        }
        let cutoff = Int64(now.timeIntervalSince1970 * 1000) - 7 * 86_400_000
        for (key, entry) in entries where !seen.contains(key) && entry.due < cutoff {
            next[key] = entry
        }
        return TermRecord(entries: next)
    }

    mutating func forget(_ task: StudentTask) {
        entries[task.uid.isEmpty ? task.id : task.uid] = nil
    }
}
