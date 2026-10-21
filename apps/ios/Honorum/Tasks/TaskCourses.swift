import Foundation

extension Library {
    /// The sections of every schedule in the open term, which is what a task's
    /// class is matched against.
    var termSections: [SavedSection] {
        schedules.filter { $0.inst == inst && $0.term == term }.flatMap(\.sections)
    }

    var taskCourses: [String] {
        Self.courses(in: termSections)
    }

    static func courses(in sections: [SavedSection]) -> [String] {
        Array(Set(sections.map(\.course))).sorted()
    }

    func course(for task: StudentTask) -> String? {
        let sections = termSections
        return Self.course(for: task, candidates: Self.courses(in: sections), sections: sections, courseMap: courseMap)
    }

    /// The matching rules, separated from the library so `TaskCourseResolver`
    /// can run them once per task rather than on every render.
    static func course(
        for task: StudentTask, candidates: [String], sections: [SavedSection], courseMap: [String: String]
    ) -> String? {
        if !task.courseEdited {
            let connection = task.extras["canvasConnection"]?.stringValue ?? ""
            let key = connection.isEmpty ? task.courseID : "\(connection):\(task.courseID)"
            if let mapped = courseMap[key] ?? courseMap[task.courseID], !mapped.isEmpty { return mapped }
        }
        if candidates.contains(task.course) { return task.course }
        let reported = (task.courseEdited ? task.course : task.courseCode + " " + task.course).uppercased()
        if let match = candidates.sorted(by: { $0.count > $1.count }).first(where: {
            Self.matches(code: $0, in: reported)
        }) { return match }
        let name = Self.titleWords(reported)
        return candidates.first { code in
            sections.filter { $0.course == code }.contains { section in
                let title = Self.titleWords(section.title)
                let (short, long) = title.count <= name.count ? (title, name) : (name, title)
                guard !short.isEmpty, short.count >= 2 || short[0].count >= 8 else { return false }
                return short.allSatisfy { a in
                    long.contains { b in
                        a == b || (a.count >= 4 && b.count >= 4 && (a.hasPrefix(b) || b.hasPrefix(a)))
                    }
                }
            }
        }
    }

    /// The same code boundaries and title matching rules as web tasks.js.
    private static func matches(code: String, in reported: String) -> Bool {
        let code = code.uppercased()
        guard let number = code.firstIndex(where: \.isNumber), number != code.startIndex else { return false }
        let subject = code[..<number].filter { $0.isASCII && $0.isLetter }
        let digits = code[number...].filter { $0.isASCII && ($0.isLetter || $0.isNumber) }
        let pattern = "(^|[^A-Z])" + subject.map(String.init).joined(separator: "[^A-Z0-9]?")
            + "[^A-Z0-9]{0,3}" + digits.map(String.init).joined(separator: "[^A-Z0-9]?") + "(?![0-9])"
        return reported.range(of: pattern, options: .regularExpression) != nil
    }

    private static let titleStopWords = Set("THE OF TO AND IN FOR A AN ON WITH I II III IV HONORS HONOR SECTION SEC LECTURE LEC DISCUSSION DISC LAB ONLINE FALL SPRING SUMMER WINTER".split(separator: " ").map(String.init))

    private static func titleWords(_ text: String) -> [String] {
        text.uppercased().split(whereSeparator: { !$0.isASCII || (!$0.isLetter && !$0.isNumber) })
            .map(String.init).filter { !titleStopWords.contains($0) && !$0.allSatisfy(\.isNumber) }
    }

    func color(for task: StudentTask) -> String {
        Self.color(for: course(for: task), in: schedule)
    }

    static func color(for course: String?, in schedule: Schedule) -> String {
        course.map { schedule.color(for: $0) } ?? CoursePalette.eventHex
    }
}

/// Matching a task to a class runs a regular expression per candidate and a
/// title comparison per section, which is far too much to repeat for every row
/// on every render of a long list. The resolver holds the term's classes once
/// and remembers each task's answer until the task's own fields change; the
/// owner replaces it when the term, its schedules, or the course map change.
@MainActor
final class TaskCourseResolver {
    struct Inputs: Equatable {
        var inst: String?
        var term: String?
        var sections: [SavedSection]
        var courseMap: [String: String]
    }

    let inputs: Inputs
    let candidates: [String]
    private var resolved: [String: String?] = [:]

    init(inputs: Inputs) {
        self.inputs = inputs
        candidates = Library.courses(in: inputs.sections)
    }

    func course(for task: StudentTask) -> String? {
        let key = Self.key(for: task)
        if let known = resolved[key] { return known }
        let answer = Library.course(
            for: task, candidates: candidates, sections: inputs.sections, courseMap: inputs.courseMap)
        resolved[key] = answer
        return answer
    }

    func color(for task: StudentTask, in schedule: Schedule) -> String {
        Library.color(for: course(for: task), in: schedule)
    }

    /// Every field the matching reads, so an edited task is matched again.
    private static func key(for task: StudentTask) -> String {
        [task.id, task.course, task.courseCode, task.courseID, task.courseEdited ? "1" : "0",
         task.extras["canvasConnection"]?.stringValue ?? ""].joined(separator: "\u{1F}")
    }
}


extension Library {
    func tasks(on date: CivilDate, in zone: TimeZone) -> [StudentTask] {
        tasks.filter { task in
            guard let due = task.dueDate else { return false }
            return CivilDate(due, in: zone) == date
        }.sorted { $0.due < $1.due }
    }

    func taskOccurrences(on date: CivilDate, in zone: TimeZone) -> [Occurrence] {
        tasks(on: date, in: zone).compactMap { task in
            guard let due = task.dueDate else { return nil }
            let end = max(30, CampusClock(due, in: zone).minute)
            return Occurrence(id: "task:" + task.id, kind: .task(task.id),
                title: task.title, subtitle: task.done ? "Completed" : "Due " + Clock.time(CampusClock(due, in: zone).minute),
                location: "", start: end - 30, end: end, colorHex: color(for: task), completed: task.done)
        }
    }
}


enum TaskSort: String, CaseIterable {
    case completion, due, title, course

    var label: String {
        switch self {
        case .completion: "Completion Date"
        case .due: "Due Date"
        case .title: "Title"
        case .course: "Class"
        }
    }

    func ordered(_ tasks: [StudentTask], library: Library, reversed: Bool) -> [StudentTask] {
        ordered(tasks, reversed: reversed) { library.course(for: $0) }
    }

    /// `course` runs once per comparison, so the caller supplies a cached lookup.
    func ordered(_ tasks: [StudentTask], reversed: Bool, course: (StudentTask) -> String?) -> [StudentTask] {
        tasks.sorted { a, b in
            let comparison: ComparisonResult
            switch self {
            case .completion, .due:
                let first = self == .completion ? a.completedAt : a.dueDate
                let second = self == .completion ? b.completedAt : b.dueDate
                // Undated tasks stay last in either direction.
                if first == nil && second != nil { return false }
                if first != nil && second == nil { return true }
                if let first, let second, first != second {
                    let before = self == .completion ? first > second : first < second
                    return reversed ? !before : before
                }
                comparison = a.title.localizedStandardCompare(b.title)
            case .title:
                comparison = a.title.localizedStandardCompare(b.title)
            case .course:
                comparison = (course(a) ?? a.course).localizedStandardCompare(course(b) ?? b.course)
            }
            if comparison == .orderedSame { return a.id < b.id }
            return reversed ? comparison == .orderedDescending : comparison == .orderedAscending
        }
    }
}
