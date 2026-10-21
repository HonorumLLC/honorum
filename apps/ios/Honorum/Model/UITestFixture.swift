#if DEBUG
import Foundation

/// Synthetic, offline data for repeatable UI tests; never uses the guest library.
enum UITestFixture {
    static var config: AppConfig {
        AppConfig(institutions: [
            Institution(id: "test", name: "Test University", timezone: "America/Chicago",
                        terms: [demo ? demoTerm : Term(id: "fall", name: "Fall 2026")],
                        closures: demo ? demoClosures : []),
            Institution(id: "other", name: "Other University", terms: [])
        ], phoneAuth: true)
    }

    private static var demo: Bool { ProcessInfo.processInfo.arguments.contains("-demoFixture") }

    /// The demo's term runs sixteen weeks around today, eight of them gone.
    private static var demoTerm: Term {
        let today = CivilDate.today(in: TimeZone(identifier: "America/Chicago")!)
        let start = today.startOfWeek().adding(days: -49)
        return Term(id: "fall", name: "Fall 2026", start: start.iso, end: start.adding(days: 16 * 7 - 3).iso)
    }

    private static var demoClosures: [Closure] {
        let today = CivilDate.today(in: TimeZone(identifier: "America/Chicago")!)
        let breakDay = today.startOfWeek().adding(days: 15)
        return [Closure(from: breakDay.iso, to: breakDay.adding(days: 1).iso, name: "Fall break")]
    }

    /// A signed-in account for `-accountFixture` screenshots.
    static var account: Account {
        let now = Date().timeIntervalSince1970
        return Account(id: "fixture", phone: "+1 405 555 0142", device: "phone", devices: [
            .init(id: "phone", name: "iPhone", created: now - 86400 * 60, lastSeen: now, push: true, current: true),
            .init(id: "mac", name: "MacBook Air", created: now - 86400 * 40, lastSeen: now - 3600 * 5, push: false, current: false),
            .init(id: "ipad", name: "iPad", created: now - 86400 * 20, lastSeen: now - 86400 * 3, push: true, current: false),
        ])
    }

    /// A term small enough to search by eye, for `-catalogFixture` runs that
    /// open a course from the class search.
    static var catalog: CatalogFile {
        func meeting(_ days: String, _ start: Int) -> Meeting {
            Meeting(days: days, start: start, end: start + 50, location: "Dale Hall")
        }
        return CatalogFile(
            inst: "test", term: Term(id: "fall", name: "Fall 2026"),
            courses: [
                Course(
                    code: "MATH 2924", subject: "MATH", title: "Differential and Integral Calculus II",
                    sections: [
                        CourseSection(
                            id: "MATH 2924", num: "001", type: "LEC", instructors: ["Ada Lovelace"],
                            meetings: [meeting("MWF", 540)], credits: 4, mode: "In person"),
                        CourseSection(
                            id: "MATH 2924-002", num: "002", type: "LEC",
                            meetings: [meeting("TR", 780)], credits: 4, mode: "In person"),
                    ]),
                Course(
                    code: "CHEM 1315", subject: "CHEM", title: "General Chemistry",
                    sections: [
                        CourseSection(
                            id: "CHEM 1315", num: "001", type: "LEC",
                            meetings: [meeting("MWF", 660)], credits: 5, mode: "In person")
                    ]),
                Course(
                    code: "PHYS 2514", subject: "PHYS", title: "General Physics for Engineering",
                    sections: [
                        CourseSection(
                            id: "PHYS 2514", num: "001", type: "LEC",
                            meetings: [meeting("TR", 900)], credits: 4, mode: "In person")
                    ]),
            ])
    }

    static var library: Library {
        if ProcessInfo.processInfo.arguments.contains("-demoFixture") { return demoLibrary }
        var library = Library(inst: "test", term: "fall",
                              onboarded: !ProcessInfo.processInfo.arguments.contains("-welcome"))
        var plan = Schedule(name: "Fall 2026", inst: "test", term: "fall", primary: true)
        let codes = ["MATH 2924", "CHEM 1315"]
        for (index, code) in codes.enumerated() {
            var section = SavedSection(section: CourseSection(id: code, num: "001"),
                                       course: Course(code: code, subject: code, title: code, sections: []))
            section.meetings = [Meeting(days: "MTWRFSU", start: 540 + index * 120, end: 590 + index * 120)]
            plan.sections.append(section)
            var task = StudentTask()
            task.id = "fixture-\(index)"
            task.title = index == 0 ? "Problem Set" : "Lab Report"
            task.course = code
            if index == 1 {
                task.source = .canvas
                task.url = "https://canvas.example.edu/courses/123/assignments/456"
            }
            task.due = ISO8601.string(from: CivilDate.today(in: TimeZone(identifier: "America/Chicago")!)
                .adding(days: 1).instant(minute: 17 * 60, in: TimeZone(identifier: "America/Chicago")!))
            library.tasks.append(task)
        }
        if ProcessInfo.processInfo.arguments.contains("-manyTasksFixture"), let template = library.tasks.first {
            for index in 1...18 {
                var task = template
                task.id = "extra-\(index)"
                task.title = "Practice Assignment \(index)"
                if let due = template.dueDate {
                    task.due = ISO8601.string(from: due.addingTimeInterval(Double(index) * 60))
                }
                library.tasks.append(task)
            }
        }
        if ProcessInfo.processInfo.arguments.contains("-dueDaysFixture"), var task = library.tasks.first {
            // A deadline the day after the others, so neighbouring days differ.
            task.id = "later"
            task.title = "Reading Response"
            if let due = task.dueDate { task.due = ISO8601.string(from: due.addingTimeInterval(86400)) }
            library.tasks.append(task)
        }
        if ProcessInfo.processInfo.arguments.contains("-scheduleGestureFixture") {
            var event = PersonalEvent()
            event.title = "Study Session"
            event.days = "MTWRFSU"
            event.start = 10 * 60
            event.end = 10 * 60 + 50
            plan.events.append(event)
        }
        if ProcessInfo.processInfo.arguments.contains("-dayPagingFixture") {
            let today = CivilDate.today(in: TimeZone(identifier: "America/Chicago")!)
            let colors = ["#cfe6fb", "#fbd7d7", "#d3f0da", "#fde6c3", "#e5d9fb"]
            for offset in -2...2 {
                for index in 0..<24 {
                    var event = PersonalEvent()
                    event.title = "Day \(offset) Event \(index)"
                    event.date = today.adding(days: offset).iso
                    event.start = 8 * 60 + index * 20
                    event.end = event.start + 45
                    event.color = colors[offset + 2]
                    plan.events.append(event)
                }
            }
        }
        library.schedule = plan
        return library
    }
    /// A realistic week for design reviews and screenshots: five classes, a
    /// couple of events, and tasks across every due bucket.
    static var demoLibrary: Library {
        let zone = TimeZone(identifier: "America/Chicago")!
        let today = CivilDate.today(in: zone)
        var library = Library(inst: "test", term: "fall", onboarded: true)
        var plan = Schedule(name: "Fall 2026", inst: "test", term: "fall", primary: true)
        let classes: [(String, String, String, [Meeting], String)] = [
            ("MATH 2924", "Calculus II", "#cfe6fb", [Meeting(days: "MWF", start: 540, end: 590, location: "Dale Hall 103")], "Ada Lovelace"),
            ("CHEM 1315", "General Chemistry", "#fbd7d7", [Meeting(days: "MWF", start: 690, end: 740, location: "Physical Sciences 120"), Meeting(days: "R", start: 870, end: 1040, location: "Chem Lab 2")], "Marie Curie"),
            ("ENGL 1213", "Composition II", "#d3f0da", [Meeting(days: "TR", start: 630, end: 705, location: "Gittinger 202")], "Toni Morrison"),
            ("PHYS 2514", "Physics for Engineers", "#fde6c3", [Meeting(days: "MWF", start: 810, end: 860, location: "Nielsen 170")], "Richard Feynman"),
            ("HIST 1483", "U.S. History to 1865", "#e5d9fb", [Meeting(days: "TR", start: 780, end: 855, location: "Dale Hall 218")], "Howard Zinn"),
        ]
        for (code, title, color, meetings, teacher) in classes {
            var section = SavedSection(
                section: CourseSection(id: code, num: "001", type: "LEC", instructors: [teacher], meetings: meetings, credits: 4, mode: "In person"),
                course: Course(code: code, subject: String(code.prefix(4)), title: title, sections: []))
            section.meetings = meetings
            if code == "MATH 2924" {
                section.exams = [Exam(date: today.adding(days: 9).iso, start: 9 * 60, end: 10 * 60 + 50, location: "Dale Hall 103")]
            }
            if code == "HIST 1483" {
                section.exams = [Exam(date: today.adding(days: 30).iso, start: 13 * 60, end: 14 * 60 + 15)]
            }
            plan.sections.append(section)
            plan.colors[code] = color
        }
        var study = PersonalEvent()
        study.title = "Study Group"
        study.location = "Bizzell Library"
        study.days = "MWF"
        study.start = 15 * 60 + 30
        study.end = 17 * 60
        plan.events.append(study)
        var shift = PersonalEvent()
        shift.title = "Work Shift"
        shift.days = "TR"
        shift.start = 17 * 60 + 30
        shift.end = 20 * 60
        plan.events.append(shift)
        library.schedule = plan
        var full = SeatWatch(inst: "test", term: "fall", section: "CHEM 1315-002", label: "CHEM 1315 · 002", title: "General Chemistry")
        full.lastAvailable = 0
        var open = SeatWatch(inst: "test", term: "fall", section: "PHYS 2514-003", label: "PHYS 2514 · 003", title: "Physics for Engineers")
        open.lastAvailable = 3
        library.watching = [full, open]

        func task(_ id: String, _ title: String, _ course: String, day: Int, minute: Int, done: Bool = false, canvas: Bool = false) -> StudentTask {
            var task = StudentTask()
            task.id = "demo-\(id)"
            task.title = title
            task.course = course
            task.done = done
            if canvas {
                task.source = .canvas
                task.url = "https://canvas.example.edu/courses/1/assignments/\(id)"
            }
            task.due = ISO8601.string(from: today.adding(days: day).instant(minute: minute, in: zone))
            if done { task.completedAt = today.adding(days: min(day, 0)).instant(minute: 12 * 60, in: zone) }
            return task
        }
        library.tasks = [
            task("1", "Reading Response 4", "ENGL 1213", day: -1, minute: 23 * 60 + 59, canvas: true),
            task("2", "Problem Set 6", "MATH 2924", day: 0, minute: 17 * 60, canvas: true),
            task("3", "Pre-lab Quiz", "CHEM 1315", day: 0, minute: 11 * 60, done: true, canvas: true),
            task("4", "Lab Notebook Check", "CHEM 1315", day: 0, minute: 23 * 60 + 59),
            task("5", "Homework 5: Kinematics", "PHYS 2514", day: 1, minute: 23 * 60 + 59, canvas: true),
            task("6", "Essay Outline", "ENGL 1213", day: 3, minute: 10 * 60 + 30),
            task("7", "Chapter 7 Reading Quiz", "HIST 1483", day: 4, minute: 13 * 60, canvas: true),
            task("8", "Lab Report 3", "CHEM 1315", day: 5, minute: 23 * 60 + 59, canvas: true),
            task("9", "Midterm Exam Review", "MATH 2924", day: 9, minute: 9 * 60),
            task("10", "Primary Source Analysis", "HIST 1483", day: 12, minute: 23 * 60 + 59),
            task("11", "Problem Set 5", "MATH 2924", day: -3, minute: 17 * 60, done: true, canvas: true),
            task("12", "Homework 4", "PHYS 2514", day: -2, minute: 23 * 60 + 59, done: true),
            task("13", "Discussion Post 6", "ENGL 1213", day: -1, minute: 12 * 60, done: true),
            task("14", "Map Quiz", "HIST 1483", day: -1, minute: 23 * 60 + 59, done: true),
        ]
        // Earlier weeks of the term, finished, so the rings differ.
        let earlier: [(String, String)] = [
            ("Problem Set 4", "MATH 2924"), ("Lab Report 2", "CHEM 1315"), ("Homework 3", "PHYS 2514"),
            ("Reading Response 3", "ENGL 1213"), ("Chapter 6 Reading Quiz", "HIST 1483"),
            ("Problem Set 3", "MATH 2924"), ("Lab Report 1", "CHEM 1315"), ("Homework 2", "PHYS 2514"),
        ]
        for (index, (title, course)) in earlier.enumerated() {
            library.tasks.append(task("past-\(index)", title, course, day: -7 - index * 3, minute: 23 * 60 + 59, done: true))
        }
        return library
    }
}
#endif
