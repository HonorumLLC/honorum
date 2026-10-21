import Foundation
import Testing

@testable import Honorum

/// Home reads the planner the way its rings count it.
@MainActor
@Suite("Home")
struct HomeModelTests {
    private static let zone = TimeZone(identifier: "America/Chicago")!
    // Tuesday, September 22, 2026, 11:20 on campus.
    private static let now = CivilDate(year: 2026, month: 9, dayOfMonth: 22).instant(minute: 11 * 60 + 20, in: zone)

    private static func at(_ day: Int, _ hour: Int = 23, _ minute: Int = 59) -> String {
        ISO8601.string(from: CivilDate(year: 2026, month: 9, dayOfMonth: day).instant(minute: hour * 60 + minute, in: zone))
    }

    private static func task(_ id: String, due: String, done: Bool = false, completed: String? = nil) -> StudentTask {
        var task = StudentTask()
        task.id = id
        task.title = id
        task.due = due
        task.done = done
        if let completed { task.extras["completedAt"] = .of(completed) }
        return task
    }

    private static func state(_ tasks: [StudentTask]) -> AppState {
        var library = Library(inst: "test", term: "fall", onboarded: true)
        library.tasks = tasks
        return AppState.fixture(
            config: AppConfig(institutions: [
                Institution(id: "test", name: "Test", timezone: "America/Chicago",
                            terms: [Term(id: "fall", name: "Fall 2026", start: "2026-08-17", end: "2026-12-11")],
                            closures: [Closure(from: "2026-10-19", to: "2026-10-20", name: "Fall break")])
            ]),
            library: library)
    }

    @Test("Today holds today's work, open overdue work, and overdue work finished today")
    func todayTasks() {
        let model = HomeModel(state: Self.state([
            Self.task("late", due: Self.at(18)),
            Self.task("lateDone", due: Self.at(17), done: true, completed: Self.at(22, 9)),
            Self.task("oldDone", due: Self.at(16), done: true, completed: Self.at(19, 9)),
            Self.task("quiz", due: Self.at(22, 17, 0)),
            Self.task("lab", due: Self.at(22)),
            Self.task("tomorrow", due: Self.at(23)),
        ]), at: Self.now)
        #expect(model.todayTasks.map(\.id) == ["lateDone", "late", "quiz", "lab"])
        #expect(model.todayTasks.filter(\.overdue).map(\.id) == ["late"])
        #expect(model.todayTasks.first { $0.id == "quiz" }?.minute == 17 * 60)
        #expect(model.counts[.today] == TaskCount(done: 1, total: 4))
    }

    @Test("The week runs Sunday to Saturday on campus dates")
    func week() {
        let model = HomeModel(state: Self.state([
            Self.task("sun", due: Self.at(20, 12), done: true),
            Self.task("tue", due: Self.at(22)),
            Self.task("sat", due: Self.at(26)),
            Self.task("next", due: Self.at(27)),
        ]), at: Self.now)
        #expect(model.week.map(\.date.dayOfMonth) == [20, 21, 22, 23, 24, 25, 26])
        #expect(model.week.map(\.tasks.count) == [1, 0, 1, 0, 0, 0, 1])
        #expect(model.week[0].count == TaskCount(done: 1, total: 1))
    }

    @Test("The term says which week it is, and its break is coming up")
    func term() throws {
        let model = HomeModel(state: Self.state([]), at: Self.now)
        let term = try #require(model.term)
        #expect(term.week == 6)
        #expect(term.weeks == 17)
        #expect(model.milestones.map(\.title) == ["Fall break"])
        #expect(model.milestones.first?.dateLabel == "Oct 19 – 20")
    }

    @Test("An empty period is complete")
    func emptyCount() {
        #expect(TaskCount(done: 0, total: 0).fraction == 1)
        #expect(TaskCount(done: 1, total: 3).percent == 33)
    }
}
