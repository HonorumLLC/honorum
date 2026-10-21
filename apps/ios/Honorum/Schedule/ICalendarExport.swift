import Foundation

/// Writes the schedule out as iCalendar, one event per meeting of the term.
/// Cancelled class days and the days the student took off do not appear.
enum ICalendarExport {
    @MainActor
    static func file(for state: AppState) -> String {
        let schedule = state.library.schedule
        let zone = state.timeZone
        let term = state.term
        let closures = state.closures
        guard let start = CivilDate(term?.start ?? ""), let end = CivilDate(term?.end ?? "") else {
            return header + footer
        }

        var lines = header
        var day = start
        while day <= end {
            for block in Occurrences.onDay(day, schedule: schedule, term: term, closures: closures) {
                let from = day.instant(minute: block.start, in: zone)
                let to = day.instant(minute: block.end, in: zone)
                lines += """
                    BEGIN:VEVENT
                    UID:\(block.id)-\(day.iso)@honorum.app
                    DTSTAMP:\(stamp(Date()))
                    DTSTART:\(stamp(from))
                    DTEND:\(stamp(to))
                    SUMMARY:\(escape(summary(block)))
                    \(block.location.isEmpty ? "" : "LOCATION:\(escape(block.location))\n")\
                    END:VEVENT

                    """
            }
            day = day.adding(days: 1)
        }
        return lines + footer
    }

    /// A plain-text week, for pasting into a message.
    @MainActor
    static func summary(for state: AppState) -> String {
        let schedule = state.library.schedule
        var lines = ["\(schedule.name) — \(state.term?.name ?? "")"]
        for section in schedule.sections.sorted(by: { $0.label < $1.label }) {
            let when = section.meetings
                .map { "\(Weekdays.describe($0.days)) \(Clock.range($0.start, $0.end))" }
                .joined(separator: ", ")
            let people = section.instructors.joined(separator: ", ")
            lines.append(
                "\(section.label) \(section.title)\(when.isEmpty ? "" : " · \(when)")"
                    + (people.isEmpty ? "" : " · \(people)"))
        }
        return lines.joined(separator: "\n")
    }

    private static func summary(_ block: Occurrence) -> String {
        block.subtitle.isEmpty ? block.title : "\(block.title) \(block.subtitle)"
    }

    private static let header = """
        BEGIN:VCALENDAR
        VERSION:2.0
        PRODID:-//Honorum//iOS//EN
        CALSCALE:GREGORIAN

        """

    private static let footer = "END:VCALENDAR\n"

    private static func stamp(_ date: Date) -> String {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(secondsFromGMT: 0)!
        let parts = calendar.dateComponents(
            [.year, .month, .day, .hour, .minute, .second], from: date)
        return String(
            format: "%04d%02d%02dT%02d%02d%02dZ", parts.year ?? 0, parts.month ?? 0,
            parts.day ?? 0, parts.hour ?? 0, parts.minute ?? 0, parts.second ?? 0)
    }

    private static func escape(_ text: String) -> String {
        text
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: ";", with: "\\;")
            .replacingOccurrences(of: ",", with: "\\,")
            .replacingOccurrences(of: "\n", with: "\\n")
    }
}
