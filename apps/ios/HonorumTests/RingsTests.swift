import Foundation
import Testing

@testable import Honorum

/// The Progress rings count the way the web's `rings.test.js` expects.
@Suite("Progress rings")
struct RingsTests {
    private static let zone = TimeZone(identifier: "America/Chicago")!
    // Tuesday, September 22, 2026, 11:20 on campus.
    private static let now = CivilDate(year: 2026, month: 9, dayOfMonth: 22).instant(minute: 11 * 60 + 20, in: zone)

    private static func at(_ day: Int, _ hour: Int = 23, _ minute: Int = 59) -> String {
        ISO8601.string(from: CivilDate(year: 2026, month: 9, dayOfMonth: day).instant(minute: hour * 60 + minute, in: zone))
    }

    private static func task(_ id: String, due: String? = nil, done: Bool = false, completed: String? = nil, course: String = "") -> StudentTask {
        var task = StudentTask()
        task.id = id
        task.due = due ?? ""
        task.done = done
        task.course = course
        if let completed { task.extras["completedAt"] = .of(completed) }
        return task
    }

    private static func counts(_ tasks: [StudentTask], weekStart: Int = 0, record: TermRecord = TermRecord(), only: String = "") -> RingCounts {
        Rings.counts(tasks: tasks, now: now, zone: zone, weekStart: weekStart, record: record, only: only) { $0.course }
    }

    @Test("Today and this week keep overdue work until it is done")
    func overdueStays() {
        let tasks = [
            Self.task("late", due: Self.at(18), course: "HIST"),  // overdue from last week
            Self.task("lateDone", due: Self.at(17), done: true, completed: Self.at(22, 9), course: "HIST"),  // finished today
            Self.task("oldDone", due: Self.at(16), done: true, completed: Self.at(19, 9)),  // finished last week
            Self.task("undated"),
            Self.task("mon", due: Self.at(21), done: true, course: "CS"),
            Self.task("tue", due: Self.at(22, 14), done: true, course: "PHYS"),
            Self.task("tue2", due: Self.at(22, 17)),
            Self.task("fri", due: Self.at(25), course: "CS"),
            Self.task("sun", due: Self.at(27)),
            Self.task("next", due: Self.at(28, 9)),
        ]
        let counts = Self.counts(tasks)
        #expect(counts.today == RingCount(done: 2, total: 4))  // late, lateDone, tue, tue2
        #expect(counts.week == RingCount(done: 3, total: 7))  // + mon, fri, sun
        // A week starting on Sunday ends on Saturday the 26th.
        #expect(Self.counts(tasks, weekStart: 6).week == RingCount(done: 3, total: 6))
        // Picking a class narrows every ring.
        #expect(Self.counts(tasks, only: "CS").week == RingCount(done: 1, total: 2))
        #expect(Self.counts(tasks, only: "CS").today == RingCount(done: 0, total: 0))
        // Tasks matched to no class are their own filter.
        #expect(Self.counts(tasks, only: "__none").today == RingCount(done: 0, total: 1))
    }

    @Test("A completion without a time counts only while its task is due in the period")
    func undatedCompletion() {
        let tasks = [Self.task("a", due: Self.at(18), done: true)]
        #expect(Self.counts(tasks).today == RingCount(done: 0, total: 0))
        #expect(RingCount(done: 0, total: 0).percent == 100)
        #expect(RingCount(done: 1, total: 3).percent == 33)
    }

    @Test("The term record keeps work that imports have dropped")
    func termRecord() {
        let start = "2026-08-24", end = "2026-12-11"
        var early = Self.task("x", due: Self.at(1), done: true, course: "CS")
        early.uid = "canvas:a"
        var recent = Self.task("y", due: Self.at(20), course: "MATH")
        recent.uid = "canvas:b"
        let manual = Self.task("z", due: Self.at(23))
        let before = Self.task(
            "summer", due: ISO8601.string(from: CivilDate(year: 2026, month: 8, dayOfMonth: 10).instant(minute: 0, in: Self.zone)), done: true)
        func update(_ record: TermRecord, _ tasks: [StudentTask]) -> TermRecord {
            record.updated(with: tasks, start: start, end: end, zone: Self.zone, now: Self.now) { $0.course }
        }
        var record = update(TermRecord(), [early, recent, manual, before])
        #expect(record.entries.keys.sorted() == ["canvas:a", "canvas:b", "z"])
        #expect(update(record, [early, recent, manual]) == record)
        // Canvas stops carrying the September 1 assignment; it stays in the record.
        // One due two days ago that disappears was removed, so it goes.
        record = update(record, [manual])
        #expect(record.entries.keys.sorted() == ["canvas:a", "z"])
        #expect(Self.counts([manual], record: record).term == RingCount(done: 1, total: 2))
        #expect(Self.counts([manual], record: record, only: "CS").term == RingCount(done: 1, total: 1))
        // A task the student deletes leaves the count, however old it is.
        var forgotten = record
        forgotten.forget(early)
        #expect(forgotten.entries.keys.sorted() == ["z"])
        // A task moved out of the term leaves it.
        var moved = manual
        moved.due = ISO8601.string(from: CivilDate(year: 2027, month: 1, dayOfMonth: 5).instant(minute: 0, in: Self.zone))
        record = update(record, [moved])
        #expect(record.entries.keys.sorted() == ["canvas:a"])
    }

    @Test("The record survives a round trip through its storage")
    func storage() {
        let defaults = UserDefaults(suiteName: "RingsTests")!
        defaults.removePersistentDomain(forName: "RingsTests")
        let key = TermRecord.key(owner: "guest", inst: "test", term: "fall")
        let record = TermRecord(entries: ["canvas:a": TermRecord.Entry(done: true, code: "CS", due: 1_700_000_000_000)])
        record.save(key, to: defaults)
        #expect(TermRecord.load(key, from: defaults) == record)
        #expect(TermRecord.load("missing", from: defaults) == TermRecord())
    }

    @Test("The streak reply decodes its days and overdue count")
    func streakDecoding() throws {
        let json = Data("""
            {"current":3,"longest":8,"today":"2026-09-22","last_day":"2026-09-22","timezone":"America/Chicago","overdue":2,
             "days":[{"day":"2026-09-21","state":"clean"},{"day":"2026-09-22","state":"overdue"}]}
            """.utf8)
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let streak = try decoder.decode(AccountStreak.self, from: json)
        #expect(streak.overdue == 2)
        #expect(streak.days?.map(\.state) == ["clean", "overdue"])
        // An older server's reply, without them, still decodes.
        let older = try decoder.decode(AccountStreak.self, from: Data("""
            {"current":3,"longest":8,"today":"2026-09-22","last_day":"","timezone":"UTC"}
            """.utf8))
        #expect(older.days == nil && older.overdue == nil)
    }
}
