import Foundation
import Testing

@testable import Honorum

@Suite("What meets on a day")
struct OccurrenceTests {
    private let term = Term(id: "202610", name: "Fall 2026", start: "2026-08-24", end: "2026-12-18")

    private func section(
        _ code: String, _ days: String, _ start: Int, _ end: Int,
        from: String? = nil, to: String? = nil
    ) -> SavedSection {
        var saved = SavedSection(
            section: CourseSection(id: code, num: "001", start: from, end: to),
            course: Course(code: code, subject: code, title: code, sections: []))
        saved.meetings = [Meeting(days: days, start: start, end: end, location: "Dale Hall")]
        return saved
    }

    @Test("Meets only on its own weekdays")
    func weekdays() {
        let mwf = section("CHEM 1315", "MWF", 660, 710)
        let thursday = CivilDate("2026-09-10")!
        let friday = CivilDate("2026-09-11")!
        #expect(Occurrences.meetings(of: mwf, on: thursday, term: term, closures: [:]).isEmpty)
        #expect(Occurrences.meetings(of: mwf, on: friday, term: term, closures: [:]).count == 1)
    }

    @Test("Stays inside the term, and inside its own dates when it has them")
    func ranges() {
        let full = section("HIST 1483", "TR", 720, 770)
        let partial = section("HIST 1493", "TR", 720, 770, from: "2026-10-01", to: "2026-10-31")
        let earlyOctober = CivilDate("2026-10-01")!
        let november = CivilDate("2026-11-05")!
        let summer = CivilDate("2026-07-02")!

        #expect(Occurrences.meetings(of: full, on: earlyOctober, term: term, closures: [:]).count == 1)
        #expect(Occurrences.meetings(of: full, on: summer, term: term, closures: [:]).isEmpty)
        #expect(Occurrences.meetings(of: partial, on: earlyOctober, term: term, closures: [:]).count == 1)
        #expect(Occurrences.meetings(of: partial, on: november, term: term, closures: [:]).isEmpty)
    }

    @Test("A closed campus cancels classes; a replacement day substitutes another weekday")
    func closures() {
        let monday = section("C S 2334", "MWF", 540, 590)
        let labor = CivilDate("2026-09-07")!
        let closed = [labor: Closure(from: "2026-09-07", to: "2026-09-07", name: "Labor Day")]
        #expect(Occurrences.meetings(of: monday, on: labor, term: term, closures: closed).isEmpty)

        let tuesday = CivilDate("2026-11-24")!
        let follows = [
            tuesday: Closure(
                from: "2026-11-24", to: "2026-11-24", name: "Monday classes meet", follows: "M")
        ]
        // The section does not meet on a Tuesday, but a Monday substitution makes it.
        #expect(Occurrences.meetings(of: monday, on: tuesday, term: term, closures: [:]).isEmpty)
        #expect(Occurrences.meetings(of: monday, on: tuesday, term: term, closures: follows).count == 1)
    }

    @Test("A closure from partway through the day cancels only the later classes")
    func partialClosure() {
        let morning = section("C S 2334", "TR", 540, 615)
        let afternoon = section("MATH 1914", "TR", 900, 975)
        let concert = CivilDate("2026-10-29")!
        let closed = [
            concert: Closure(from: "2026-10-29", to: "2026-10-29", name: "Concert", after: 900)
        ]
        #expect(Occurrences.meetings(of: morning, on: concert, term: term, closures: closed).count == 1)
        #expect(Occurrences.meetings(of: afternoon, on: concert, term: term, closures: closed).isEmpty)
    }

    @Test("Overlapping blocks are laid out side by side, and separate ones are not")
    func columns() {
        var schedule = Schedule()
        schedule.sections = [
            section("A", "R", 600, 700),
            section("B", "R", 650, 750),
            section("C", "R", 900, 960),
        ]
        let blocks = Occurrences.onDay(
            CivilDate("2026-09-10")!, schedule: schedule, term: term, closures: [:])

        #expect(blocks.count == 3)
        #expect(blocks.first?.colorHex == CoursePalette.hexes[0])
        let overlapping = blocks.filter { $0.start < 800 }
        #expect(overlapping.allSatisfy { $0.columns == 2 })
        #expect(Set(overlapping.map(\.column)) == [0, 1])
        #expect(blocks.last?.columns == 1)
    }

    @Test("A weekly personal event and a one-off event land on the right days")
    func personalEvents() {
        var schedule = Schedule()
        var weekly = PersonalEvent()
        weekly.id = "shift"
        weekly.title = "Work shift"
        weekly.days = "R"
        weekly.start = 1020
        weekly.end = 1200
        var once = PersonalEvent()
        once.id = "advising"
        once.title = "Advising"
        once.date = "2026-09-11"
        once.start = 600
        once.end = 630
        schedule.events = [weekly, once]

        let thursday = Occurrences.onDay(
            CivilDate("2026-09-10")!, schedule: schedule, term: term, closures: [:])
        let friday = Occurrences.onDay(
            CivilDate("2026-09-11")!, schedule: schedule, term: term, closures: [:])

        #expect(thursday.map(\.title) == ["Work shift"])
        #expect(friday.map(\.title) == ["Advising"])
    }

    @Test("An event takes its class's color, else its own label, else the neutral fill")
    func eventColors() {
        var schedule = Schedule()
        schedule.sections = [section("MATH 2924", "MWF", 600, 650)]
        var review = PersonalEvent()
        review.id = "review"
        review.title = "Review session"
        review.date = "2026-09-11"
        review.course = "MATH 2924"
        review.color = "#fbd7d7"  // ignored beside a class
        var gym = PersonalEvent()
        gym.id = "gym"
        gym.title = "Gym"
        gym.date = "2026-09-11"
        gym.start = 700
        gym.end = 760
        gym.color = "#d3f0da"
        var lunch = PersonalEvent()
        lunch.id = "lunch"
        lunch.title = "Lunch"
        lunch.date = "2026-09-11"
        lunch.start = 800
        lunch.end = 830
        schedule.events = [review, gym, lunch]

        let friday = Occurrences.onDay(
            CivilDate("2026-09-11")!, schedule: schedule, term: term, closures: [:])
        let byTitle = Dictionary(uniqueKeysWithValues: friday.map { ($0.title, $0) })
        #expect(byTitle["Review session"]?.colorHex == CoursePalette.hexes[0])
        #expect(byTitle["Review session"]?.subtitle == "MATH 2924")
        #expect(byTitle["Gym"]?.colorHex == "#d3f0da")
        #expect(byTitle["Lunch"]?.colorHex == nil)
    }

    @Test("Exam sittings land on their date in the class's color, survive closures, and can be hidden")
    func exams() {
        var schedule = Schedule()
        var cs = section("C S 2334", "MWF", 540, 590)
        cs.exams = [
            Exam(date: "2026-10-08", start: 1140, end: 1260, location: "Dale Hall 200"),
            Exam(date: "2026-12-17", start: 480, end: 600),
        ]
        schedule.sections = [cs]
        let midterm = CivilDate("2026-10-08")!  // a Thursday, when the class does not meet
        let closed = [midterm: Closure(from: "2026-10-08", to: "2026-10-08", name: "Reading Day")]
        let day = Occurrences.onDay(midterm, schedule: schedule, term: term, closures: closed)
        #expect(day.map(\.title) == ["C S 2334 Exam"])
        #expect(day.first?.colorHex == CoursePalette.hexes[0])
        #expect(day.first?.location == "Dale Hall 200")
        #expect(day.first?.itemID == "C S 2334:exam:0")
        if case .exam(let sectionID, _) = day.first?.kind {
            #expect(sectionID == "C S 2334")
        } else {
            Issue.record("expected an exam occurrence")
        }
        let quiet = Occurrences.onDay(
            CivilDate("2026-10-09")!, schedule: schedule, term: term, closures: [:])
        #expect(quiet.count == 1)  // Friday: only the lecture

        schedule.removed["C S 2334:exam:1"] = ["2026-12-17"]
        let finalDay = Occurrences.onDay(
            CivilDate("2026-12-17")!, schedule: schedule, term: term, closures: [:])
        #expect(finalDay.isEmpty)
    }
}

@Suite("The saved plan")
struct PlanTests {
    @Test("Web schedules without color overrides use ordered unique courses")
    func colors() {
        var schedule = Schedule()
        schedule.sections = [saved("a", "C S 2334"), saved("b", "C S 2334"), saved("c", "MATH 2924")]
        #expect(schedule.color(for: "C S 2334") == CoursePalette.hexes[0])
        #expect(schedule.color(for: "MATH 2924") == CoursePalette.hexes[1])
        #expect(schedule.colors.isEmpty)
        schedule.colors["MATH 2924"] = "#123456"
        #expect(schedule.color(for: "MATH 2924") == "#123456")
        schedule.sections.removeAll { $0.course == "C S 2334" }
        #expect(schedule.color(for: "MATH 2924") == "#123456")
        schedule.colors = [:]
        #expect(schedule.color(for: "MATH 2924") == CoursePalette.hexes[0])
    }

    private func saved(_ id: String, _ code: String) -> SavedSection {
        SavedSection(section: CourseSection(id: id, num: "001"),
                     course: Course(code: code, subject: code, title: code, sections: []))
    }

    @Test("Task mapping and manual overrides use the same colors as the calendar")
    func taskColors() {
        var library = Library(inst: "ou", term: "fall")
        var plan = Schedule(name: "Fall", inst: "ou", term: "fall", primary: true)
        plan.sections = [saved("a", "C S 2334"), saved("b", "MATH 2924")]
        library.schedule = plan
        var task = StudentTask()
        task.courseCode = "MATH-2924-001"
        #expect(library.course(for: task) == "MATH 2924")
        #expect(library.color(for: task) == CoursePalette.hexes[1])
        task.courseID = "42"
        library.courseMap["42"] = "C S 2334"
        #expect(library.color(for: task) == CoursePalette.hexes[0])
        task.courseEdited = true
        task.course = "MATH 2924"
        #expect(library.color(for: task) == CoursePalette.hexes[1])
        task.course = ""
        #expect(library.course(for: task) == nil)
        #expect(library.color(for: task) == CoursePalette.eventHex)
    }

    @Test("Imported names match web code boundaries and course titles")
    func importedNames() {
        var library = Library(inst: "ou", term: "fall")
        var plan = Schedule(name: "Fall", inst: "ou", term: "fall", primary: true)
        var philosophy = saved("phil", "PHIL 1273")
        philosophy.title = "Introduction to Business Ethics"
        plan.sections = [saved("cs", "CS 101"), philosophy]
        library.schedule = plan
        var task = StudentTask()
        task.course = "PHYSICS 1010"
        #expect(library.course(for: task) == nil)
        task.course = "2026FA-CS-101-010"
        #expect(library.course(for: task) == "CS 101")
        task.course = "Intro to Business Ethics (Honors)"
        #expect(library.course(for: task) == "PHIL 1273")
    }

    @Test("Writing the current schedule replaces it in place")
    func currentSchedule() {
        var library = Library()
        var plan = Schedule(name: "Fall", inst: "ou", term: "202610", primary: true)
        plan.id = "abc"
        library.schedule = plan
        plan.name = "Spring"
        library.schedule = plan

        #expect(library.schedules.count == 1)
        #expect(library.schedule.name == "Spring")
    }

    @Test("Unknown seat counts stay unknown rather than reading as full")
    func seats() {
        let file = SeatsFile(
            inst: "ou", term: "202610", seats: ["1": [100, 12, 0], "2": [30, -1, 4], "3": [10, 0, 2]])
        let table = file.table()
        #expect(table["1"]?.isOpen == true)
        #expect(table["2"]?.isKnown == false)
        #expect(table["3"]?.isKnown == true)
        #expect(table["3"]?.isOpen == false)
        #expect(table["3"]?.waitlist == 2)
    }
}


@Suite("Calendar task deadlines")
struct TaskCalendarTests {
    @Test("Deadlines use campus days and clamp midnight to the first half hour")
    func midnight() {
        var library = Library()
        var task = StudentTask()
        task.title = "Exam"
        task.done = true
        task.due = "2026-09-15T05:00:00Z"
        library.tasks = [task]
        let zone = TimeZone(identifier: "America/Chicago")!
        let date = CivilDate("2026-09-15")!
        let blocks = library.taskOccurrences(on: date, in: zone)
        #expect(blocks.count == 1)
        #expect(blocks.first?.start == 0)
        #expect(blocks.first?.end == 30)
        #expect(blocks.first?.completed == true)
        #expect(library.tasks(on: date.adding(days: -1), in: zone).isEmpty)
    }

    @Test("Tasks share columns with meetings without changing class conflicts")
    func overlapping() {
        var library = Library()
        var task = StudentTask()
        task.due = "2026-09-15T15:00:00Z"
        library.tasks = [task]
        let blocks = library.taskOccurrences(on: CivilDate("2026-09-15")!, in: TimeZone(identifier: "America/Chicago")!)
        let meeting = Occurrence(id: "class", kind: .section("class"), title: "Class",
            subtitle: "", location: "", start: 570, end: 630)
        let laidOut = Occurrences.layOut((blocks + [meeting]).sorted { ($0.start, $0.end) < ($1.start, $1.end) })
        #expect(laidOut.count == 2)
        #expect(laidOut.allSatisfy { $0.columns == 2 })
        #expect(Set(laidOut.map(\.column)) == [0, 1])
    }

    @Test("Connection-specific course choices override legacy choices")
    func courseMapping() {
        var library = Library()
        var task = StudentTask()
        task.courseID = "42"
        task.extras["canvasConnection"] = .of("new-site")
        library.courseMap = ["42": "OLD 100", "new-site:42": "NEW 200"]
        #expect(library.course(for: task) == "NEW 200")
        task.extras["canvasConnection"] = .of("another-site")
        #expect(library.course(for: task) == "OLD 100")
        task.courseEdited = true
        #expect(library.course(for: task) != "OLD 100")
    }
}


@Suite("Task completion history and sorting")
struct CompletionHistoryTests {
    @Test("Completion dates survive sync, keep subsecond order, and reset when reopened")
    func history() throws {
        let instant = Date(timeIntervalSince1970: 1_800_000_000.125)
        var task = StudentTask()
        task.done = true
        task.recordCompletion(previousDone: false, at: instant)
        let restored = StudentTask(task.body)
        #expect(abs(try #require(restored.completedAt).timeIntervalSince(instant)) < 0.001)
        task.recordCompletion(previousDone: true, at: instant.addingTimeInterval(60))
        #expect(task.completedAt == restored.completedAt)
        task.done = false
        task.recordCompletion(previousDone: true)
        #expect(task.extras["completedAt"] == nil)
        task.done = true
        task.recordCompletion(previousDone: false, at: instant.addingTimeInterval(120))
        #expect(task.completedAt! > restored.completedAt!)
    }

    @Test("Newest completion comes first; undated tasks stay last in either direction")
    func sorting() {
        var first = StudentTask()
        first.id = "first"
        first.title = "Zebra"
        first.done = true
        first.completedAt = Date(timeIntervalSince1970: 100.125)
        var second = first
        second.id = "second"
        second.title = "Apple"
        second.completedAt = Date(timeIntervalSince1970: 100.875)
        var legacy = first
        legacy.id = "legacy"
        legacy.completedAt = nil
        let tasks = [legacy, first, second]
        #expect(TaskSort.completion.ordered(tasks, library: Library(), reversed: false).map(\.id) == ["second", "first", "legacy"])
        #expect(TaskSort.completion.ordered(tasks, library: Library(), reversed: true).map(\.id) == ["first", "second", "legacy"])
        #expect(TaskSort.title.ordered([first, second], library: Library(), reversed: false).map(\.id) == ["second", "first"])
    }
}
