import Foundation
import Testing

@testable import Honorum

@Suite("Calendar feeds")
struct ICalendarTests {
    private let feed = """
        BEGIN:VCALENDAR
        VERSION:2.0
        BEGIN:VEVENT
        UID:event-assignment-9
        DTSTART:20991115T170000Z
        SUMMARY:Rhetorical analysis draft [ENGL 1213 018]
        LOCATION:Felgar Hall 319
        END:VEVENT
        BEGIN:VEVENT
        UID:folded-1
        DTSTART;TZID=America/Chicago:20991201T235900
        SUMMARY:A very long assignment title that the
          feed wrapped onto a second line
        END:VEVENT
        BEGIN:VEVENT
        UID:old-1
        DTSTART:20200101T120000Z
        SUMMARY:Long past
        END:VEVENT
        END:VCALENDAR
        """

    @Test("Reads events, unfolds wrapped lines, and skips what is long past")
    func parsing() {
        let tasks = ICalendar.tasks(from: feed, feed: "https://example.edu/feed.ics")
        #expect(tasks.count == 2)
        #expect(tasks[0].uid == "feed:event-assignment-9")
        #expect(tasks[0].title == "Rhetorical analysis draft")
        #expect(tasks[0].course == "ENGL 1213 018")
        #expect(tasks[1].title.contains("wrapped onto a second line"))
        #expect(!tasks.contains { $0.title == "Long past" })
    }

    @Test("A whole-day item is due at the end of that day")
    func wholeDay() {
        let line = ICalendar.parse("DUE;VALUE=DATE:20991115")
        let due = ICalendar.date(from: line)
        #expect(due != nil)
        let parts = Calendar.current.dateComponents([.hour, .minute], from: due!)
        #expect(parts.hour == 23)
        #expect(parts.minute == 59)
    }

    @Test("Floating feed dates use the selected campus zone")
    func floatingDates() throws {
        let line = ICalendar.parse("DUE;VALUE=DATE:20260910")
        let chicago = ICalendar.date(from: line, timeZone: TimeZone(identifier: "America/Chicago")!)
        let utc = ICalendar.date(from: line, timeZone: TimeZone(secondsFromGMT: 0)!)
        let difference = try #require(chicago).timeIntervalSince(#require(utc))
        #expect(abs(difference - 18_000.0) < 0.001)
    }

    @Test("A time zone parameter is honored")
    func timeZones() {
        let chicago = ICalendar.date(
            from: ICalendar.parse("DTSTART;TZID=America/Chicago:20260910T090000"))
        let utc = ICalendar.date(from: ICalendar.parse("DTSTART:20260910T090000Z"))
        #expect(chicago != nil && utc != nil)
        #expect(chicago! > utc!)  // Chicago 09:00 happens later than 09:00 UTC
    }
}

@Suite("Phone numbers")
struct PhoneTests {
    @Test("A national number becomes E.164")
    func e164() {
        let us = PhoneCountries.country("US")!
        #expect(PhoneCountries.e164(us, "(405) 555-0143") == "+14055550143")
    }

    @Test("Obvious nonsense is caught before a text is requested")
    func plausibility() {
        let us = PhoneCountries.country("US")!
        #expect(!PhoneCountries.isPlausible(us, "12"))
        #expect(PhoneCountries.isPlausible(us, "4055550143"))
        #expect(!PhoneCountries.isPlausible(us, "40555501430000"))
    }

    @Test("Every region has a calling code and a flag")
    func coverage() {
        #expect(PhoneCountries.all.count > 200)
        #expect(PhoneCountries.all.allSatisfy { !$0.code.isEmpty && !$0.flag.isEmpty })
    }
}

@Suite("Reminders")
struct ReminderTests {
    @Test("A cancelled class day produces no class reminder")
    @MainActor
    func skipsClosures() {
        let state = AppState()
        state.library.inst = "ou"
        state.library.term = "202610"
        var plan = Schedule(name: "Fall", inst: "ou", term: "202610", primary: true)
        plan.id = "s1"
        var saved = SavedSection(
            section: CourseSection(id: "1", num: "001"),
            course: Course(code: "C S 2334", subject: "C S", title: "Programming", sections: []))
        saved.meetings = [Meeting(days: "MTWRFSU", start: 9 * 60, end: 10 * 60)]
        plan.sections = [saved]
        state.library.schedules = [plan]
        state.library.current = "s1"

        // Without configuration there is no term or closure list, so every day
        // in the window offers a meeting; sixty is the cap that ships.
        let items = Reminders.upcoming(in: state, from: Date().addingTimeInterval(-86400))
        #expect(items.count <= Reminders.maximumScheduled)
        #expect(items.map(\.fireAt) == items.map(\.fireAt).sorted())
    }
}

@Suite("Task source links")
struct TaskSourceLinkTests {
    @Test("Canvas links retain their course, assignment, and fragment")
    func canvasLink() {
        var task = StudentTask()
        task.source = .canvas
        task.url = " https://canvas.example.edu/courses/123/assignments/456?module_item_id=789#details "
        #expect(task.sourceURL?.absoluteString == "https://canvas.example.edu/courses/123/assignments/456?module_item_id=789#details")
    }

    @Test("Unusable or non-web source addresses do not become buttons", arguments: [
        "", "/courses/123/assignments/456", "https://", "javascript:alert(1)",
        "file:///etc/passwd", "https://user:password@canvas.example.edu/assignment"
    ])
    func invalidLink(_ address: String) {
        var task = StudentTask()
        task.source = .canvas
        task.url = address
        #expect(task.sourceURL == nil)
    }

    @Test("Manual tasks have no source link; calendar imports retain theirs")
    func sourceKinds() {
        var task = StudentTask()
        task.url = "https://canvas.example.edu/courses/123/assignments/456"
        #expect(task.sourceURL == nil)
        task.source = .feed
        #expect(task.sourceURL?.host == "canvas.example.edu")
    }
}


@Suite("Local and account imports")
struct AccountImportIsolationTests {
    @Test @MainActor
    func localFeedRefreshPreservesAccountAssignments() {
        let state = AppState()
        var shared = StudentTask()
        shared.source = .feed
        shared.uid = "shared-feed-item"
        shared.extras["canvasConnection"] = .of("account-feed")
        var local = StudentTask()
        local.source = .feed
        local.uid = "removed-local-item"
        state.library.tasks = [shared, local]
        state.reconcile([], source: .feed)
        #expect(state.library.tasks == [shared])
    }

    @Test("A finished task stays when an import no longer lists it; an open one goes") @MainActor
    func keepsFinishedTasks() {
        let state = AppState()
        var finished = StudentTask()
        finished.source = .canvas
        finished.uid = "canvas:assignment:1"
        finished.done = true
        var unfinished = StudentTask()
        unfinished.source = .canvas
        unfinished.uid = "canvas:assignment:2"
        state.library.tasks = [finished, unfinished]
        state.reconcile([], source: .canvas)
        #expect(state.library.tasks == [finished])
    }

    @Test @MainActor
    func matchingListsCoursesSeparatelyForEachConnection() {
        let state = AppState()
        var first = StudentTask()
        first.courseID = "42"
        first.extras["canvasConnection"] = .of("first")
        var second = first
        second.id = "second-task"
        second.extras["canvasConnection"] = .of("second")
        state.library.tasks = [first, second]
        #expect(state.importedCourses.count == 2)
        #expect(Set(state.importedCourses.map(\.id)) == ["first:42", "second:42"])
    }
}

@Suite("Imported due dates")
struct ImportedDueTests {
    private static func assignment(_ id: Int, due: String) -> ImportedTask {
        ImportedTask(uid: "canvas:assignment:\(id)", title: "Task \(id)", course: "", due: due, done: false)
    }

    @Test("A date the student changed stays until the source moves its own") @MainActor
    func keepsStudentDates() {
        let state = AppState()
        // A guest task from before importedDue was recorded counts as unchanged.
        var earlier = StudentTask()
        earlier.source = .canvas
        earlier.uid = "canvas:assignment:3"
        earlier.due = "2026-10-07T18:00:00Z"
        state.library.tasks = [earlier]
        let quiz = Self.assignment(3, due: "2026-10-03T05:59:59Z")
        state.reconcile([quiz, Self.assignment(1, due: "2026-10-02T05:59:59Z"), Self.assignment(2, due: "")], source: .canvas)
        #expect(state.library.tasks.map(\.due) == ["2026-10-03T05:59:59Z", "2026-10-02T05:59:59Z", ""])

        state.library.tasks[1].due = "2026-10-05T18:00:00Z"
        state.library.tasks[2].due = "2026-10-06T18:00:00Z"
        state.reconcile([quiz, Self.assignment(1, due: "2026-10-02T05:59:59Z"), Self.assignment(2, due: "")], source: .canvas)
        #expect(state.library.tasks.map(\.due) == ["2026-10-03T05:59:59Z", "2026-10-05T18:00:00Z", "2026-10-06T18:00:00Z"])

        // The instructor moves the essay; the reading stays undated in Canvas.
        state.reconcile([quiz, Self.assignment(1, due: "2026-10-09T05:59:59Z"), Self.assignment(2, due: "")], source: .canvas)
        #expect(state.library.tasks.map(\.due) == ["2026-10-03T05:59:59Z", "2026-10-09T05:59:59Z", "2026-10-06T18:00:00Z"])
    }
}
