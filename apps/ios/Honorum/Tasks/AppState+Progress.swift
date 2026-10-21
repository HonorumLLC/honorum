import Foundation

/// The Progress card's numbers: ring counts for the class the task filter
/// picks, and the term record they draw on, kept on this device for each
/// account, school, and term.
extension AppState {
    /// A task's class the way the record names it: the class the resolver
    /// matched it to, or nothing.
    private func ringCode(_ task: StudentTask, _ resolver: TaskCourseResolver) -> String {
        resolver.course(for: task) ?? ""
    }

    func ringCounts(now: Date = Date(), only: String = "") -> RingCounts {
        let resolver = taskCourses
        return Rings.counts(
            tasks: library.tasks, now: now, zone: timeZone, record: termRecord(now: now), only: only
        ) { ringCode($0, resolver) }
    }

    /// The term record brought up to date with the current tasks, saved when
    /// it changed. Reading it is cheap enough to do on every render.
    func termRecord(now: Date = Date()) -> TermRecord {
        let key = TermRecord.key(owner: owner, inst: library.inst ?? "", term: library.term ?? "")
        let known = termRecordCache?.key == key ? termRecordCache?.record : nil
        let record = known ?? TermRecord.load(key)
        let resolver = taskCourses
        let next = record.updated(
            with: library.tasks, start: term?.start, end: term?.end, zone: timeZone, now: now
        ) { ringCode($0, resolver) }
        if next != record || known == nil {
            if next != record { next.save(key) }
            termRecordCache = (key, next)
        }
        return next
    }

    /// A task the student deletes leaves the count, however old it is.
    func forgetTermTask(_ task: StudentTask) {
        let key = TermRecord.key(owner: owner, inst: library.inst ?? "", term: library.term ?? "")
        var record = termRecordCache?.key == key ? termRecordCache!.record : TermRecord.load(key)
        record.forget(task)
        record.save(key)
        termRecordCache = (key, record)
    }
}
