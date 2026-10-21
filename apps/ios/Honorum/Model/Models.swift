import Foundation

// MARK: - Public configuration

/// `GET /api/config`. Describes the schools this deployment collects, their
/// terms, and the immutable catalog publications available for each term.
struct AppConfig: Codable, Sendable, Hashable {
    var institutions: [Institution] = []
    var version: String?
    var phoneAuth: Bool?
    /// The server can deliver account alerts and reminders to iPhones itself.
    var apns: Bool?
}

struct Institution: Codable, Sendable, Hashable, Identifiable {
    var id: String
    var name: String
    var timezone: String?
    var lms: LMS?
    var terms: [Term] = []
    var closures: [Closure] = []

    var timeZone: TimeZone { timezone.flatMap(TimeZone.init(identifier:)) ?? .current }

    /// Closures keyed by civil date, expanded from the inclusive ranges the
    /// server publishes, so a day lookup is a dictionary hit.
    func closureIndex() -> [CivilDate: Closure] {
        var index: [CivilDate: Closure] = [:]
        for closure in closures {
            guard var day = CivilDate(closure.from) else { continue }
            let last = CivilDate(closure.to) ?? day
            while day <= last {
                index[day] = closure
                day = day.adding(days: 1)
            }
        }
        return index
    }
}

struct LMS: Codable, Sendable, Hashable {
    var kind: String
    var url: String?

    var providerName: String { kind == "canvas" ? "Canvas" : kind.capitalized }
}

struct Term: Codable, Sendable, Hashable, Identifiable {
    var id: String
    var name: String
    var start: String?
    var end: String?
    var sections: Int?
    /// Content hashes of the term's current publications; absent until the
    /// term has been collected.
    var structureSnapshot: String?
    var seatsSnapshot: String?
}

/// A school holiday, break, or replacement class day. `follows` substitutes
/// another weekday's classes onto this date; `after` cancels only the classes
/// starting at or after that campus minute.
struct Closure: Codable, Sendable, Hashable {
    var from: String
    var to: String
    var name: String
    var follows: String?
    var after: Int?

    /// Whether the closure cancels a class meeting starting at `start`.
    func cancels(start: Int) -> Bool {
        guard let after, after > 0 else { return true }
        return start >= after
    }
}

// MARK: - Catalog

/// One published `.structure.<hash>.json` file. Sections omit anything that
/// repeats the course, term, or institution; `CatalogIndex` fills those in.
struct CatalogFile: Codable, Sendable {
    var inst: String
    var term: Term
    var url: String?
    var courses: [Course]
    var hash: String?
}

struct Course: Codable, Sendable, Hashable, Identifiable {
    var code: String
    var subject: String
    var title: String
    var offered: [String]?
    var sections: [CourseSection]

    var id: String { code }
}

struct CourseSection: Codable, Sendable, Hashable, Identifiable {
    var id: String
    var num: String
    var type: String?
    var instructors: [String]?
    var meetings: [Meeting]?
    var credits: Double?
    var start: String?
    var end: String?
    var mode: String?
    var link: String?
    var attributes: [String]?
    var exams: [Exam]?

    var teachers: [String] { instructors ?? [] }
    var days: [Meeting] { meetings ?? [] }
    var sittings: [Exam] { exams ?? [] }
}

struct Meeting: Codable, Sendable, Hashable {
    /// Weekday letters from `MTWRFSU`, Monday first.
    var days: String
    /// Minutes after midnight, campus time.
    var start: Int
    var end: Int
    var location: String?

    init(days: String, start: Int, end: Int, location: String? = nil) {
        self.days = days
        self.start = start
        self.end = end
        self.location = location
    }

    init(_ body: JSON) {
        days = body.text("days")
        start = body.int("start")
        end = body.int("end")
        location = body.text("location")
    }

    var body: JSON {
        .object([
            "days": .of(days), "start": .of(start), "end": .of(end),
            "location": .of(location ?? ""),
        ])
    }
}

/// One dated sitting a school publishes with a section: a midterm or final.
struct Exam: Codable, Sendable, Hashable {
    /// A civil date, campus time.
    var date: String
    /// Minutes after midnight, campus time.
    var start: Int
    var end: Int
    var location: String?

    init(date: String, start: Int, end: Int, location: String? = nil) {
        self.date = date
        self.start = start
        self.end = end
        self.location = location
    }

    init(_ body: JSON) {
        date = body.text("date")
        start = body.int("start")
        end = body.int("end")
        location = body.text("location")
    }

    var body: JSON {
        .object([
            "date": .of(date), "start": .of(start), "end": .of(end),
            "location": .of(location ?? ""),
        ])
    }
}

/// Seat counts from a published `.seats.<hash>.json` file, or from `/api/seats`.
struct Seats: Sendable, Hashable, Codable {
    var capacity: Int
    var available: Int
    var waitlist: Int

    /// The server publishes `-1` when a count is unknown rather than zero.
    var isKnown: Bool { available >= 0 }
    var isOpen: Bool { available > 0 }
}

struct SeatsFile: Codable, Sendable {
    var inst: String
    var term: String
    var seats: [String: [Int]]

    func table() -> [String: Seats] {
        seats.reduce(into: [:]) { table, entry in
            let counts = entry.value
            table[entry.key] = Seats(
                capacity: counts.count > 0 ? counts[0] : 0,
                available: counts.count > 1 ? counts[1] : -1,
                waitlist: counts.count > 2 ? counts[2] : 0)
        }
    }
}

// MARK: - The student's plan
//
// These types are the shapes the account's documents already use, because the
// web client reads and writes the same documents. Each one keeps the fields it
// did not recognize in `extras` so an edit here never drops them.

/// A section saved into a schedule. It keeps its own copy of everything the
/// planner draws so a launch renders the week before any network request.
struct SavedSection: Codable, Sendable, Hashable, Identifiable {
    var id: String
    var course: String
    var subject: String
    var title: String
    var num: String
    var type: String
    var link: String
    var instructors: [String]
    var meetings: [Meeting]
    /// Dated exam sittings the school published with the section.
    var exams: [Exam] = []
    var credits: Double
    var start: String
    var end: String
    var mode: String
    var capacity: Int
    var available: Int
    var waitlist: Int
    var url: String
    var extras: [String: JSON] = [:]

    static let knownKeys: Set<String> = [
        "id", "course", "subject", "title", "num", "type", "link", "meetings", "exams", "credits",
        "start", "end", "instructors", "mode", "capacity", "available", "waitlist", "url",
    ]

    init(section: CourseSection, course: Course, catalogURL: String = "", seats: Seats? = nil) {
        id = section.id
        self.course = course.code
        subject = course.subject
        title = course.title
        num = section.num
        type = section.type ?? ""
        link = section.link ?? ""
        instructors = section.teachers
        meetings = section.days
        exams = section.sittings
        credits = section.credits ?? 0
        start = section.start ?? ""
        end = section.end ?? ""
        mode = section.mode ?? ""
        capacity = seats?.capacity ?? 0
        available = seats?.available ?? -1
        waitlist = seats?.waitlist ?? 0
        url = catalogURL
    }

    init(_ body: JSON) {
        id = body.text("id")
        course = body.text("course")
        subject = body.text("subject")
        title = body.text("title")
        num = body.text("num")
        type = body.text("type")
        link = body.text("link")
        instructors = body.list("instructors").compactMap(\.stringValue)
        meetings = body.list("meetings").map(Meeting.init)
        exams = body.list("exams").map(Exam.init)
        credits = body["credits"]?.numberValue ?? 0
        start = body.text("start")
        end = body.text("end")
        mode = body.text("mode")
        capacity = body.int("capacity")
        available = body["available"]?.intValue ?? -1
        waitlist = body.int("waitlist")
        url = body.text("url")
        extras = body.extras(besides: Self.knownKeys)
    }

    var body: JSON {
        JSON.object([
            "id": .of(id), "course": .of(course), "subject": .of(subject), "title": .of(title),
            "num": .of(num), "type": .of(type), "link": .of(link),
            "meetings": .array(meetings.map(\.body)), "exams": .array(exams.map(\.body)),
            "credits": .of(credits),
            "start": .of(start), "end": .of(end),
            "instructors": .array(instructors.map(JSON.of)), "mode": .of(mode),
            "capacity": .of(capacity), "available": .of(available), "waitlist": .of(waitlist),
            "url": .of(url),
        ]).merging(extras: extras)
    }

    var label: String { "\(course)-\(num)" }
    var seats: Seats { Seats(capacity: capacity, available: available, waitlist: waitlist) }
}

/// A student's own event: a job shift, a study block, a club meeting. The
/// account calls these blocks.
struct PersonalEvent: Codable, Sendable, Hashable, Identifiable {
    var id: String = UUID().uuidString
    var title: String = ""
    var location: String = ""
    /// Weekday letters when the event repeats weekly; empty for a single date.
    var days: String = ""
    /// A civil date when the event happens once.
    var date: String = ""
    var start: Int = 9 * 60
    var end: Int = 10 * 60
    var notes: String = ""
    /// The schedule's class this event belongs to, which decides its color.
    var course: String = ""
    /// A palette hex chosen for an event without a class; empty for the neutral fill.
    var color: String = ""
    var extras: [String: JSON] = [:]

    static let knownKeys: Set<String> = [
        "id", "name", "days", "date", "start", "end", "location", "note", "course", "color",
    ]

    init() {}

    init(_ body: JSON) {
        id = body.text("id")
        title = body.text("name")
        days = body.text("days")
        date = body.text("date")
        start = body.int("start")
        end = body.int("end")
        location = body.text("location")
        notes = body.text("note")
        course = body.text("course")
        color = body.text("color")
        extras = body.extras(besides: Self.knownKeys)
    }

    /// A one-time event also carries its date's weekday letter, as the web
    /// writes it: the web planner and the server's reminders place an event
    /// only on the days it lists.
    var body: JSON {
        let days = CivilDate(date).map { String($0.letter) } ?? days
        return JSON.object([
            "id": .of(id), "name": .of(title), "days": .of(days), "date": .of(date),
            "start": .of(start), "end": .of(end), "location": .of(location), "note": .of(notes),
            "course": .of(course), "color": .of(color),
        ]).merging(extras: extras)
    }

    var repeatsWeekly: Bool { date.isEmpty }

    /// Mirrors web blockColor: the class's color, else the label, else neutral.
    func colorHex(in schedule: Schedule) -> String? {
        if !course.isEmpty { return schedule.color(for: course) }
        return color.isEmpty ? nil : color
    }
}

struct Schedule: Codable, Sendable, Hashable, Identifiable {
    var id: String = UUID().uuidString.lowercased()
    var name: String = "My Schedule"
    var inst: String = ""
    var term: String = ""
    var primary: Bool = true
    var sections: [SavedSection] = []
    var events: [PersonalEvent] = []
    /// Dates a single meeting of a section or event was taken off the plan.
    var removed: [String: [String]] = [:]
    /// Course code to a palette color, as the hex the web app stores.
    var colors: [String: String] = [:]
    var extras: [String: JSON] = [:]

    static let knownKeys: Set<String> = [
        "id", "name", "inst", "term", "primary", "sections", "pinned", "removed", "blocks", "colors",
    ]

    init() {}

    init(name: String, inst: String, term: String, primary: Bool) {
        self.name = name
        self.inst = inst
        self.term = term
        self.primary = primary
    }

    init(_ body: JSON) {
        id = body.text("id")
        name = body.text("name")
        inst = body.text("inst")
        term = body.text("term")
        primary = body.flag("primary")
        removed = body.map("removed").mapValues { $0.arrayValue?.compactMap(\.stringValue) ?? [] }
        colors = body.map("colors").compactMapValues(\.stringValue)
        events = body.list("blocks").map(PersonalEvent.init)
        // The account stores an ordered list of ids beside a map of the section
        // copies, so a schedule keeps its order and its offline detail.
        let pinned = body.map("pinned")
        sections = body.list("sections").compactMap(\.stringValue).compactMap {
            pinned[$0].map(SavedSection.init)
        }
        extras = body.extras(besides: Self.knownKeys)
    }

    var body: JSON {
        JSON.object([
            "id": .of(id), "name": .of(name), "inst": .of(inst), "term": .of(term),
            "primary": .of(primary),
            "sections": .array(sections.map { .of($0.id) }),
            "pinned": .object(Dictionary(uniqueKeysWithValues: sections.map { ($0.id, $0.body) })),
            "removed": .object(removed.mapValues { .array($0.map(JSON.of)) }),
            "blocks": .array(events.map(\.body)),
            "colors": .object(colors.mapValues(JSON.of)),
        ]).merging(extras: extras)
    }

    var credits: Double { sections.reduce(0) { $0 + $1.credits } }

    /// The schedule's course codes in the order they were added, once each.
    var courses: [String] {
        var codes: [String] = []
        for section in sections where !codes.contains(section.course) {
            codes.append(section.course)
        }
        return codes
    }

    /// Mirrors web colorOf: explicit overrides, then ordered unique courses.
    func color(for course: String) -> String {
        if let existing = colors[course] { return existing }
        var codes: [String] = []
        for section in sections where !codes.contains(section.course) {
            codes.append(section.course)
        }
        return CoursePalette.hexes[(codes.firstIndex(of: course) ?? 0) % CoursePalette.hexes.count]
    }

}

/// A manual or imported assignment.
struct StudentTask: Codable, Sendable, Hashable, Identifiable {
    enum Source: String, Codable, Sendable { case manual, canvas, feed }

    var id: String = UUID().uuidString.lowercased()
    var title: String = ""
    var course: String = ""
    /// RFC 3339, or empty when the task has no due time.
    var due: String = ""
    var notes: String = ""
    var done: Bool = false
    var source: Source = .manual
    /// The upstream identity, such as `canvas:assignment:12345`.
    var uid: String = ""
    var courseID: String = ""
    var courseCode: String = ""
    var url: String = ""

    /// Only web destinations from imported tasks can be opened as source links.
    var sourceURL: URL? {
        guard source != .manual,
              let link = URL(string: url.trimmingCharacters(in: .whitespacesAndNewlines)),
              let scheme = link.scheme?.lowercased(), ["https", "http"].contains(scheme),
              let host = link.host, !host.isEmpty,
              link.user == nil, link.password == nil else { return nil }
        return link
    }

    /// Canvas Student accepts the same host, path, query and fragment as the web.
    var canvasAppURL: URL? {
        guard source == .canvas, let link = sourceURL,
              var components = URLComponents(url: link, resolvingAgainstBaseURL: false)
        else { return nil }
        components.scheme = "canvas-courses"
        return components.url
    }

    var overrideID: String = ""
    var moduleItem: String = ""
    /// A completion change that still has to reach Canvas.
    var pendingDone: Bool?
    var pendingCanvasDone: String?
    var courseEdited: Bool = false
    var extras: [String: JSON] = [:]

    static let knownKeys: Set<String> = [
        "id", "title", "course", "due", "note", "done", "source", "uid", "courseId",
        "courseCode", "url", "overrideId", "moduleItem", "pendingDone", "pendingCanvasDone", "courseEdited",
    ]

    init() {}

    init(_ body: JSON) {
        id = body.text("id")
        title = body.text("title")
        course = body.text("course")
        due = body.text("due")
        notes = body.text("note")
        done = body.flag("done")
        source = Source(rawValue: body.text("source")) ?? .manual
        uid = body.text("uid")
        courseID = body.text("courseId")
        courseCode = body.text("courseCode")
        url = body.text("url")
        overrideID = body.text("overrideId")
        moduleItem = body.text("moduleItem")
        pendingDone = body["pendingDone"]?.boolValue
        pendingCanvasDone = body["pendingCanvasDone"]?.stringValue
        courseEdited = body.flag("courseEdited")
        extras = body.extras(besides: Self.knownKeys)
    }

    var body: JSON {
        var fields: [String: JSON] = [
            "id": .of(id), "title": .of(title), "course": .of(course), "due": .of(due),
            "note": .of(notes), "done": .of(done), "source": .of(source.rawValue),
        ]
        for (key, value) in [
            "uid": uid, "courseId": courseID, "courseCode": courseCode, "url": url,
            "overrideId": overrideID, "moduleItem": moduleItem,
        ] where !value.isEmpty {
            fields[key] = .of(value)
        }
        if courseEdited { fields["courseEdited"] = .of(true) }
        if let pendingDone { fields["pendingDone"] = .of(pendingDone) }
        if let pendingCanvasDone { fields["pendingCanvasDone"] = .of(pendingCanvasDone) }
        return JSON.object(fields).merging(extras: extras)
    }

    var dueDate: Date? { due.isEmpty ? nil : ISO8601.date(from: due) }

    /// Additive document metadata; older clients retain it with unknown fields.
    var completedAt: Date? {
        get { done ? extras["completedAt"]?.stringValue.flatMap(ISO8601.date(from:)) : nil }
        set { extras["completedAt"] = newValue.map { .of($0.formatted(Date.ISO8601FormatStyle(includingFractionalSeconds: true))) } }
    }

    /// The due date the source last reported, so an import can tell a date the
    /// student changed from one the source moved. Nil before the first import
    /// that records it.
    var importedDue: String? {
        get { extras["importedDue"]?.stringValue }
        set { extras["importedDue"] = newValue.map { .of($0) } }
    }

    mutating func recordCompletion(previousDone: Bool, at date: Date = Date()) {
        guard done != previousDone else { return }
        completedAt = done ? date : nil
    }

    /// The Canvas plannable this task came from, for a completion write.
    var canvasPlannable: (type: String, id: String)? {
        let parts = uid.split(separator: ":", maxSplits: 2).map(String.init)
        guard parts.count == 3, parts[0] == "canvas" else { return nil }
        return (parts[1], parts[2])
    }
}

/// A section the student is watching for an open seat.
struct SeatWatch: Codable, Sendable, Hashable, Identifiable {
    var inst: String
    var term: String
    var section: String
    var label: String
    var title: String
    /// The count last seen, so the app only announces a change to open.
    var lastAvailable: Int = -1

    var id: String { "\(inst)/\(term)/\(section)" }

    init(inst: String, term: String, section: String, label: String, title: String) {
        self.inst = inst
        self.term = term
        self.section = section
        self.label = label
        self.title = title
    }

    init?(_ body: JSON) {
        guard let object = body.objectValue, object["section"] != nil else { return nil }
        inst = body.text("inst")
        term = body.text("term")
        section = body.text("section")
        label = body.text("label")
        title = body.text("title")
        lastAvailable = body["lastAvailable"]?.intValue ?? -1
    }

    var body: JSON {
        .object([
            "inst": .of(inst), "term": .of(term), "section": .of(section),
            "label": .of(label), "title": .of(title), "lastAvailable": .of(lastAvailable),
        ])
    }
}

/// A seat alert the server holds for this account, created from any client.
struct ServerWatch: Codable, Sendable, Hashable, Identifiable {
    var id: String
    var inst: String
    var term: String
    var section: String
    var label: String?
    var title: String?
    var channel: String?
}

// MARK: - The local library

/// Everything the app keeps for one owner — the account, or the guest who has
/// not signed in — written as one atomic envelope.
struct Library: Codable, Sendable {
    var inst: String?
    var term: String?
    var schedules: [Schedule] = []
    var current: String?
    var tasks: [StudentTask] = []
    var onboarded: Bool = false
    var reminders: Bool = false
    /// LMS course id to the class code the student matched it to.
    var courseMap: [String: String] = [:]
    var watching: [SeatWatch] = []
    /// Preference fields written by another client.
    var prefExtras: [String: JSON] = [:]

    static let prefKeys: Set<String> = [
        "inst", "term", "current", "onboarded", "reminders", "map", "watching",
    ]

    var schedule: Schedule {
        get {
            schedules.first { $0.id == current }
                ?? schedules.first { $0.primary }
                ?? schedules.first
                ?? Schedule()
        }
        set {
            if let index = schedules.firstIndex(where: { $0.id == newValue.id }) {
                schedules[index] = newValue
            } else {
                schedules.append(newValue)
                current = newValue.id
            }
        }
    }

    /// Classes, events, tasks, or seat alerts: a planner already in use, which
    /// has no need of the setup pages.
    var holdsPlans: Bool {
        !tasks.isEmpty || !watching.isEmpty
            || schedules.contains { !$0.sections.isEmpty || !$0.events.isEmpty }
    }

    /// Schedules for the term now open, which is what the planner switches between.
    var termSchedules: [Schedule] {
        schedules.filter { $0.inst == (inst ?? "") && $0.term == (term ?? "") }
    }

    var prefsBody: JSON {
        JSON.object([
            "inst": .of(inst ?? ""), "term": .of(term ?? ""), "current": .of(current ?? ""),
            "onboarded": .of(onboarded), "reminders": .of(reminders),
            "map": .object(courseMap.mapValues(JSON.of)),
            "watching": .array(watching.map(\.body)),
        ]).merging(extras: prefExtras)
    }

    /// Applies a preferences document. Navigation is restored only on the
    /// first download after signing in, never by a later background poll.
    mutating func apply(prefs body: JSON, restoringNavigation: Bool) {
        if body.flag("onboarded") { onboarded = true }
        reminders = body.flag("reminders")
        courseMap = body.map("map").compactMapValues(\.stringValue)
        watching = body.list("watching").compactMap(SeatWatch.init)
        prefExtras = body.extras(besides: Self.prefKeys)
        guard restoringNavigation else { return }
        if !body.text("inst").isEmpty { inst = body.text("inst") }
        if !body.text("term").isEmpty { term = body.text("term") }
        if !body.text("current").isEmpty { current = body.text("current") }
    }
}

/// RFC 3339 timestamps, the form the account documents and the LMS relay use.
enum ISO8601 {
    private static let plain = Date.ISO8601FormatStyle()
    private static let fractional = Date.ISO8601FormatStyle(includingFractionalSeconds: true)

    static func date(from text: String) -> Date? {
        (try? plain.parse(text)) ?? (try? fractional.parse(text))
    }

    static func string(from date: Date) -> String { plain.format(date) }
}
