import SwiftUI

/// What Home shows, read from the planner once per render: the three ring
/// counts, today's tasks and classes, the days ahead, and the term. Every
/// date is a campus date and every time a campus minute, so the page reads
/// the same wherever the phone is.
struct HomeDay: Identifiable, Hashable {
    let date: CivilDate
    let tasks: [TaskItem]

    var id: Int { date.day }
    var count: TaskCount { TaskCount(done: tasks.filter(\.done).count, total: tasks.count) }
}

struct HomeMilestone: Identifiable, Hashable {
    let id: String
    let title: String
    let colorHex: String?
    let from: CivilDate
    let to: CivilDate

    var dateLabel: String {
        from == to ? from.mediumDay
            : from.month == to.month ? "\(from.monthDay) – \(to.dayOfMonth)"
            : "\(from.monthDay) – \(to.monthDay)"
    }
}

struct HomeCourseItem: Identifiable, Hashable {
    let code: String
    let title: String
    let colorHex: String
    let count: TaskCount
    let next: String?

    var id: String { code }
}

struct HomeTerm: Hashable {
    let name: String
    let start: CivilDate
    let end: CivilDate
    let today: CivilDate

    var weeks: Int { max(1, (end.day - start.day + 7) / 7) }
    var week: Int { min(weeks, max(1, (today.day - start.day) / 7 + 1)) }
    func position(_ date: CivilDate) -> Double {
        min(1, max(0, Double(date.day - start.day) / Double(max(1, end.day - start.day))))
    }
}

@MainActor
struct HomeModel {
    let today: CivilDate
    let now: Int
    let counts: [RingPeriod: TaskCount]
    let todayTasks: [TaskItem]
    /// What is due in the seven days after today, soonest first.
    let upcoming: [TaskItem]
    let classes: [ClassItem]
    let week: [HomeDay]
    let term: HomeTerm?
    let milestones: [HomeMilestone]
    let courses: [HomeCourseItem]

    init(state: AppState, at instant: Date = Date()) {
        let zone = state.timeZone
        let clock = CampusClock(instant, in: zone)
        today = clock.date
        now = clock.minute
        let schedule = state.schedule
        let resolver = state.taskCourses

        let rings = state.ringCounts(now: instant)
        counts = [
            .today: TaskCount(done: rings.today.done, total: rings.today.total),
            .week: TaskCount(done: rings.week.done, total: rings.week.total),
            .term: TaskCount(done: rings.term.done, total: rings.term.total),
        ]

        let today = today
        func item(_ task: StudentTask) -> TaskItem {
            TaskItem(task: task, colorHex: resolver.color(for: task, in: schedule), zone: zone, today: today)
        }
        let dated = state.library.tasks.filter { $0.dueDate != nil }.map(item)
            .sorted { ($0.date?.day ?? 0, $0.minute) < ($1.date?.day ?? 0, $1.minute) }

        // Today counts the way its ring does: what is due today, anything
        // overdue that is still open, and overdue work finished today.
        todayTasks = dated.filter { item in
            guard let date = item.date else { return false }
            if date == today { return true }
            if date > today { return false }
            if !item.done { return true }
            return item.task.completedAt.map { CivilDate($0, in: zone) >= today } ?? false
        }
        let horizon = today.adding(days: 7)
        upcoming = dated.filter { item in item.date.map { $0 > today && $0 <= horizon } ?? false }

        classes = ClassItem.list(state, on: today)

        let first = today.startOfWeek()
        week = (0..<7).map { offset in
            let date = first.adding(days: offset)
            return HomeDay(date: date, tasks: dated.filter { $0.date == date })
        }

        if let current = state.term, let start = CivilDate(current.start ?? ""),
           let end = CivilDate(current.end ?? ""), end > start {
            term = HomeTerm(name: current.name, start: start, end: end, today: today)
        } else {
            term = nil
        }

        // What is ahead in the term: the school's breaks (not an afternoon's
        // cancellation) and the exams it published for the saved sections.
        var ahead: [HomeMilestone] = []
        let last = term?.end ?? today.adding(days: 120)
        for closure in state.institution?.closures ?? [] where closure.follows == nil && (closure.after ?? 0) == 0 {
            guard let from = CivilDate(closure.from) else { continue }
            let to = CivilDate(closure.to) ?? from
            guard to >= today, from <= last else { continue }
            ahead.append(HomeMilestone(id: "closure-\(closure.from)", title: closure.name,
                                       colorHex: nil, from: from, to: to))
        }
        for section in schedule.sections {
            for (index, exam) in section.exams.enumerated() {
                guard let date = CivilDate(exam.date), date >= today, date <= last else { continue }
                ahead.append(HomeMilestone(id: "\(section.id)-exam-\(index)", title: "\(section.title.isEmpty ? section.course : section.title) exam",
                                           colorHex: schedule.color(for: section.course), from: date, to: date))
            }
        }
        milestones = Array(ahead.sorted { $0.from < $1.from }.prefix(6))

        // Each class's share of the term, from the record the term ring
        // counts, and the next open task matched to it.
        let record = state.termRecord(now: instant)
        var seen: Set<String> = []
        var tiles: [HomeCourseItem] = []
        for section in schedule.sections where seen.insert(section.course).inserted {
            let entries = record.entries.values.filter { $0.code == section.course }
            let next = dated.first { !$0.done && !$0.overdue && resolver.course(for: $0.task) == section.course }
            tiles.append(HomeCourseItem(
                code: section.course, title: section.title.isEmpty ? section.course : section.title,
                colorHex: schedule.color(for: section.course),
                count: TaskCount(done: entries.filter(\.done).count, total: entries.count),
                next: next?.title))
        }
        courses = tiles
    }
}
