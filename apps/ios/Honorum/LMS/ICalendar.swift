import Foundation

/// A very small iCalendar reader: enough to turn an assignment feed into
/// tasks. Feeds come through the server relay, which checks the destination
/// and bounds the response; the parsing happens here on the device.
enum ICalendar {
    /// Assignments from one feed. Items due more than a fortnight ago are
    /// dropped, matching what the web client keeps. `label` names the class
    /// for items whose summary does not carry one.
    static func tasks(from text: String, feed: String, label: String = "", now: Date = Date(), timeZone: TimeZone = .current) -> [ImportedTask] {
        let cutoff = now.addingTimeInterval(-14 * 86400)
        var out: [ImportedTask] = []
        for component in components(in: unfold(text)) {
            guard component.name == "VEVENT" || component.name == "VTODO" else { continue }
            let summary = component["SUMMARY"]?.value.trimmingCharacters(in: .whitespaces) ?? ""
            guard !summary.isEmpty else { continue }
            let (title, course) = split(summary, label: label.isEmpty ? (component["CATEGORIES"]?.value ?? "") : label)
            let dueLine = component["DUE"] ?? component["DTSTART"] ?? component["DTEND"]
            guard let dueLine, let due = date(from: dueLine, timeZone: timeZone) else { continue }
            guard due >= cutoff else { continue }
            let uid = component["UID"]?.value ?? "\(feed)#\(title)#\(dueLine.value)"
            let status = component["STATUS"]?.value.uppercased()
            out.append(
                ImportedTask(
                    uid: "feed:\(uid)",
                    title: title,
                    course: course,
                    courseId: nil,
                    courseCode: nil,
                    due: ISO8601.string(from: due),
                    done: status == "COMPLETED",
                    url: component["URL"]?.value,
                    overrideId: nil,
                    moduleItem: nil,
                    reason: nil))
        }
        return out
    }

    /// Canvas and Blackboard put the class in the summary's trailing bracket,
    /// as in "Essay 2 [ENGL 1213 018]": the title is the part before it.
    static func split(_ summary: String, label: String) -> (title: String, course: String) {
        guard summary.hasSuffix("]"), let open = summary.lastIndex(of: "[") else { return (summary, label) }
        let inside = summary[summary.index(after: open)..<summary.index(before: summary.endIndex)]
            .trimmingCharacters(in: .whitespaces)
        guard !inside.isEmpty else { return (summary, label) }
        return (String(summary[..<open]).trimmingCharacters(in: .whitespaces), inside)
    }

    // MARK: Reading the format

    struct Line {
        var name: String
        var parameters: [String: String]
        var value: String
    }

    struct Component {
        var name: String
        var lines: [Line]
        subscript(name: String) -> Line? { lines.first { $0.name == name } }
    }

    /// Continuation lines start with a space or tab and belong to the line above.
    static func unfold(_ text: String) -> [String] {
        var out: [String] = []
        for raw in text.split(whereSeparator: \.isNewline) {
            if raw.hasPrefix(" ") || raw.hasPrefix("\t") {
                out[out.isEmpty ? out.startIndex : out.index(before: out.endIndex)] +=
                    raw.dropFirst()
            } else {
                out.append(String(raw))
            }
        }
        return out
    }

    static func components(in lines: [String]) -> [Component] {
        var out: [Component] = []
        var open: Component?
        for raw in lines {
            let line = parse(raw)
            switch (line.name, line.value) {
            case ("BEGIN", let name) where name != "VCALENDAR":
                open = Component(name: name, lines: [])
            case ("END", _):
                if let component = open { out.append(component) }
                open = nil
            default:
                open?.lines.append(line)
            }
        }
        return out
    }

    static func parse(_ raw: String) -> Line {
        guard let colon = raw.firstIndex(of: ":") else {
            return Line(name: raw.uppercased(), parameters: [:], value: "")
        }
        let head = raw[raw.startIndex..<colon]
        let value = String(raw[raw.index(after: colon)...])
            .replacingOccurrences(of: "\\n", with: "\n")
            .replacingOccurrences(of: "\\,", with: ",")
            .replacingOccurrences(of: "\\;", with: ";")
            .replacingOccurrences(of: "\\\\", with: "\\")
        var parts = head.split(separator: ";")
        let name = String(parts.removeFirst()).uppercased()
        var parameters: [String: String] = [:]
        for part in parts {
            let pair = part.split(separator: "=", maxSplits: 1)
            if pair.count == 2 {
                parameters[String(pair[0]).uppercased()] =
                    String(pair[1]).trimmingCharacters(in: CharacterSet(charactersIn: "\""))
            }
        }
        return Line(name: name, parameters: parameters, value: value)
    }

    /// `20260910T170000Z`, `20260910T170000` with a TZID, or a bare `20260910`.
    static func date(from line: Line, timeZone: TimeZone = .current) -> Date? {
        let text = line.value
        var calendar = Calendar(identifier: .gregorian)
        let zone =
            line.parameters["TZID"].flatMap(TimeZone.init(identifier:))
            ?? (text.hasSuffix("Z") ? TimeZone(secondsFromGMT: 0)! : timeZone)
        calendar.timeZone = zone

        let digits = text.filter(\.isNumber)
        guard digits.count >= 8 else { return nil }
        func number(_ from: Int, _ length: Int) -> Int? {
            let start = digits.index(digits.startIndex, offsetBy: from)
            let end = digits.index(start, offsetBy: length)
            return Int(digits[start..<end])
        }
        var components = DateComponents()
        components.year = number(0, 4)
        components.month = number(4, 2)
        components.day = number(6, 2)
        if digits.count >= 14 {
            components.hour = number(8, 2)
            components.minute = number(10, 2)
            components.second = number(12, 2)
        } else {
            // A whole-day item is due at the end of that day.
            components.hour = 23
            components.minute = 59
        }
        return calendar.date(from: components)
    }
}
