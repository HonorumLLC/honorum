import Foundation

/// A civil date in the campus time zone, stored as a day number so paging a
/// week, comparing a term range, or laying out a month costs integer math
/// instead of a `Calendar` round trip. Dates on this screen are never elapsed
/// milliseconds: a class at 9:00 is at 9:00 on campus wherever the phone is.
struct CivilDate: Hashable, Comparable, Sendable, Codable {
    /// Days since 1970-01-01.
    var day: Int

    init(day: Int) { self.day = day }

    init(year: Int, month: Int, dayOfMonth: Int) {
        let y = year - (month <= 2 ? 1 : 0)
        let era = (y >= 0 ? y : y - 399) / 400
        let yoe = y - era * 400
        let doy = (153 * (month + (month > 2 ? -3 : 9)) + 2) / 5 + dayOfMonth - 1
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy
        day = era * 146097 + doe - 719468
    }

    /// Parses `YYYY-MM-DD` without a formatter.
    init?(_ iso: String) {
        let parts = iso.split(separator: "-", omittingEmptySubsequences: false)
        guard parts.count >= 3, let y = Int(parts[0]), let m = Int(parts[1]),
            let d = Int(parts[2].prefix(2)), (1...12).contains(m), (1...31).contains(d)
        else { return nil }
        self.init(year: y, month: m, dayOfMonth: d)
    }

    init(_ instant: Date, in zone: TimeZone) {
        let shifted = instant.timeIntervalSince1970 + Double(zone.secondsFromGMT(for: instant))
        day = Int(floor(shifted / 86400))
    }

    private var parts: (year: Int, month: Int, day: Int) {
        var z = day + 719468
        let era = (z >= 0 ? z : z - 146096) / 146097
        z -= era * 146097
        let yoe = (z - z / 1460 + z / 36524 - z / 146096) / 365
        let doy = z - (365 * yoe + yoe / 4 - yoe / 100)
        let mp = (5 * doy + 2) / 153
        let d = doy - (153 * mp + 2) / 5 + 1
        let m = mp + (mp < 10 ? 3 : -9)
        return (yoe + era * 400 + (m <= 2 ? 1 : 0), m, d)
    }

    var year: Int { parts.year }
    var month: Int { parts.month }
    var dayOfMonth: Int { parts.day }

    /// 0 is Monday, matching the catalog's `MTWRFSU` meeting letters.
    var weekdayIndex: Int { ((day + 3) % 7 + 7) % 7 }
    var letter: Character { Array("MTWRFSU")[weekdayIndex] }

    var iso: String {
        let p = parts
        return "\(p.year)-\(p.month < 10 ? "0" : "")\(p.month)-\(p.day < 10 ? "0" : "")\(p.day)"
    }

    func adding(days: Int) -> CivilDate { CivilDate(day: day + days) }

    /// `weekStart` is 0 for Monday, 6 for Sunday.
    func startOfWeek(_ weekStart: Int = 6) -> CivilDate {
        adding(days: -(((weekdayIndex - weekStart) % 7 + 7) % 7))
    }

    /// The instant a campus-local minute of this date occurs.
    func instant(minute: Int, in zone: TimeZone) -> Date {
        let naive = Double(day) * 86400 + Double(minute) * 60
        // One correction resolves the offset on the meeting date itself, which
        // matters on the two days a year a time zone shifts.
        let guess = Date(timeIntervalSince1970: naive - Double(zone.secondsFromGMT()))
        return Date(timeIntervalSince1970: naive - Double(zone.secondsFromGMT(for: guess)))
    }

    static func < (a: CivilDate, b: CivilDate) -> Bool { a.day < b.day }

    static func today(in zone: TimeZone) -> CivilDate { CivilDate(Date(), in: zone) }
}

/// The campus clock: the current civil date and minute where the school is.
struct CampusClock: Sendable {
    var date: CivilDate
    var minute: Int

    init(_ instant: Date = Date(), in zone: TimeZone) {
        let shifted = instant.timeIntervalSince1970 + Double(zone.secondsFromGMT(for: instant))
        let day = floor(shifted / 86400)
        date = CivilDate(day: Int(day))
        minute = Int((shifted - day * 86400) / 60)
    }
}

// MARK: - Occurrences

/// One drawn block on a day: a class meeting or a personal event.
struct Occurrence: Identifiable, Hashable, Sendable {
    /// An exam carries its section's id and its own item id, which is what a
    /// hidden sitting is keyed by (the web's `<section>:exam:<index>`).
    enum Kind: Hashable, Sendable { case section(String), event(String), task(String), exam(section: String, item: String) }

    var id: String
    var kind: Kind
    var title: String
    var subtitle: String
    var location: String
    var start: Int
    var end: Int
    var colorHex: String?
    var completed = false

    /// Fraction of the row's width, assigned when meetings overlap.
    var date: CivilDate?
    var column = 0
    var columns = 1

    /// The item this block belongs to, which is what a removed day is keyed by.
    var itemID: String {
        switch kind {
        case .section(let id), .event(let id), .task(let id): id
        case .exam(_, let item): item
        }
    }
}

enum Occurrences {
    /// Meetings of one saved section on a civil date, honoring the term range,
    /// the section's own dates, campus closures, and replacement class days.
    static func meetings(
        of section: SavedSection, on date: CivilDate, term: Term?,
        closures: [CivilDate: Closure], removed: [String] = []
    ) -> [Meeting] {
        guard !removed.contains(date.iso) else { return [] }
        let from = CivilDate(section.start) ?? CivilDate(term?.start ?? "")
        let to = CivilDate(section.end) ?? CivilDate(term?.end ?? "")
        if let from, date < from { return [] }
        if let to, date > to { return [] }
        var letter = date.letter
        let closure = closures[date]
        if let substitute = closure?.follows?.first {
            letter = substitute
        }
        return section.meetings.filter {
            $0.days.contains(letter) && $0.end > $0.start
                && !(closure?.follows == nil && closure?.cancels(start: $0.start) == true)
        }
    }

    /// Everything drawn on one day, laid out into columns where blocks overlap:
    /// class meetings, the exam sittings published with them (dated, so
    /// closures never cancel them), and the student's own events.
    static func onDay(
        _ date: CivilDate, schedule: Schedule, term: Term?, closures: [CivilDate: Closure]
    ) -> [Occurrence] {
        var blocks: [Occurrence] = []
        for section in schedule.sections {
            let color = schedule.color(for: section.course)
            let dropped = schedule.removed[section.id] ?? []
            for meeting in Occurrences.meetings(
                of: section, on: date, term: term, closures: closures, removed: dropped)
            {
                blocks.append(
                    Occurrence(
                        id: "\(section.id)-\(meeting.start)", kind: .section(section.id),
                        title: section.course, subtitle: section.title,
                        location: meeting.location ?? "", start: meeting.start, end: meeting.end,
                        colorHex: color))
            }
            for (index, exam) in section.exams.enumerated() {
                let item = "\(section.id):exam:\(index)"
                guard CivilDate(exam.date) == date, exam.end > exam.start,
                    !(schedule.removed[item] ?? []).contains(date.iso)
                else { continue }
                blocks.append(
                    Occurrence(
                        id: item, kind: .exam(section: section.id, item: item),
                        title: "\(section.course) Exam", subtitle: section.label,
                        location: exam.location ?? "", start: exam.start, end: exam.end,
                        colorHex: color))
            }
        }
        for event in schedule.events {
            let matches =
                event.repeatsWeekly
                ? event.days.contains(date.letter) : CivilDate(event.date) == date
            guard matches, event.end > event.start,
                !(schedule.removed[event.id] ?? []).contains(date.iso)
            else { continue }
            blocks.append(
                Occurrence(
                    id: event.id, kind: .event(event.id), title: event.title,
                    subtitle: event.course, location: event.location, start: event.start, end: event.end,
                    colorHex: event.colorHex(in: schedule)))
        }
        return layOut(blocks.sorted { ($0.start, $0.end) < ($1.start, $1.end) }).map { block in
            var dated = block
            dated.date = date
            return dated
        }
    }

    /// Assigns overlapping blocks to side-by-side columns, the way Calendar
    /// splits a busy hour, so nothing is hidden behind anything else.
    static func layOut(_ blocks: [Occurrence]) -> [Occurrence] {
        var result = blocks
        var cluster: [Int] = []
        var clusterEnd = Int.min

        func close() {
            guard !cluster.isEmpty else { return }
            var ends: [Int] = []
            for index in cluster {
                let column = ends.firstIndex { $0 <= result[index].start } ?? ends.count
                if column == ends.count { ends.append(0) }
                ends[column] = result[index].end
                result[index].column = column
            }
            for index in cluster { result[index].columns = ends.count }
            cluster = []
        }

        for index in result.indices {
            if result[index].start >= clusterEnd { close() }
            cluster.append(index)
            clusterEnd = max(clusterEnd, result[index].end)
        }
        close()
        return result
    }
}
