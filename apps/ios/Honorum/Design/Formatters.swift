import Foundation

/// Formatting helpers that build their formatters once. Creating a
/// `DateFormatter` consults the locale and time zone database, which is far too
/// much work to repeat for every row of a calendar.
enum Clock {
    private static let hour: DateFormatter = template("j")
    private static let hourMinute: DateFormatter = template("j:mm")
    private static let minuteOnly: DateFormatter = template("mm")

    private static func template(_ pattern: String) -> DateFormatter {
        let formatter = DateFormatter()
        formatter.locale = .autoupdatingCurrent
        formatter.timeZone = TimeZone(secondsFromGMT: 0)
        formatter.setLocalizedDateFormatFromTemplate(pattern)
        return formatter
    }

    private static func reference(_ minute: Int) -> Date {
        Date(timeIntervalSince1970: Double(minute % (24 * 60)) * 60)
    }

    /// "9:30 AM", or "09:30" where the locale prefers a 24-hour clock.
    static func time(_ minute: Int) -> String {
        hourMinute.string(from: reference(minute))
    }

    /// The gutter label for a whole hour: "9 AM", "Noon", "13".
    static func hourLabel(_ hour: Int) -> String {
        if hour == 12 { return "Noon" }
        return self.hour.string(from: reference(hour * 60))
    }

    static func range(_ start: Int, _ end: Int) -> String {
        "\(time(start)) – \(time(end))"
    }

    static func duration(_ minutes: Int) -> String {
        let hours = minutes / 60, rest = minutes % 60
        if hours == 0 { return "\(rest) min" }
        if rest == 0 { return hours == 1 ? "1 hour" : "\(hours) hours" }
        return "\(hours) hr \(rest) min"
    }

    /// Whether the locale shows AM/PM, which decides if the gutter needs a suffix.
    static var usesDayPeriod: Bool { hour.dateFormat?.contains("a") ?? false }

    /// The gutter label split into its number and its day-period suffix, so the
    /// suffix can be set smaller the way Calendar sets it.
    static func hourParts(_ value: Int) -> (String, String) {
        if value == 12 { return ("Noon", "") }
        guard usesDayPeriod else { return (String(format: "%02d", value), "") }
        let number = value % 12 == 0 ? 12 : value % 12
        let suffix = value < 12 ? (hour.amSymbol ?? "AM") : (hour.pmSymbol ?? "PM")
        return ("\(number)", suffix)
    }
}

enum Weekdays {
    static let letters = "MTWRFSU"

    /// Single-letter column headings in the reader's locale, Sunday first.
    static let initials: [String] = {
        let symbols = DateFormatter().veryShortStandaloneWeekdaySymbols ?? ["S", "M", "T", "W", "T", "F", "S"]
        return symbols
    }()

    /// Names for the day letters the catalog uses, in `MTWRFSU` order.
    static let names = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]
    static let shortNames = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]

    /// "MWF", "TR", or "Mon Wed Fri" for a meeting pattern.
    static func describe(_ days: String) -> String {
        let order = letters.map { $0 }
        let present = order.enumerated().filter { days.contains($0.element) }
        return present.map { shortNames[$0.offset] }.joined(separator: " ")
    }
}

extension CivilDate {
    /// Formats this civil date without dragging in a time zone conversion.
    func formatted(_ style: Date.FormatStyle) -> String {
        var components = DateComponents()
        components.year = year
        components.month = month
        components.day = dayOfMonth
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(secondsFromGMT: 0)!
        let date = calendar.date(from: components) ?? Date()
        var fixed = style
        fixed.timeZone = calendar.timeZone
        return date.formatted(fixed)
    }

    var monthAndYear: String { formatted(.dateTime.month(.wide).year()) }
    var monthTitle: String { formatted(.dateTime.month(.wide)) }
    var longDay: String { formatted(.dateTime.weekday(.wide).month(.abbreviated).day().year()) }
    var weekdayName: String { formatted(.dateTime.weekday(.wide)) }
    var shortDate: String { formatted(.dateTime.month(.abbreviated).day().year()) }
    var monthDay: String { formatted(.dateTime.month(.abbreviated).day()) }
    var mediumDay: String { formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day()) }
}
