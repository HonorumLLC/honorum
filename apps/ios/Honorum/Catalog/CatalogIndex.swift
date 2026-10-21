import Foundation
import os

/// Filters over the term's sections, mirroring the web app's class search.
struct CatalogFilters: Equatable, Sendable {
    enum TimeWindow: String, CaseIterable, Sendable {
        case any, morning, afternoon, evening

        var label: String {
            switch self {
            case .any: "Any time"
            case .morning: "Morning"
            case .afternoon: "Afternoon"
            case .evening: "Evening"
            }
        }

        var range: ClosedRange<Int>? {
            switch self {
            case .any: nil
            case .morning: 0...(12 * 60 - 1)
            case .afternoon: (12 * 60)...(17 * 60 - 1)
            case .evening: (17 * 60)...(24 * 60)
            }
        }
    }

    var openOnly = false
    /// Weekday letters the section must stay within, empty for any.
    var days = ""
    var time = TimeWindow.any
    var mode = ""
    var level = ""

    var isDefault: Bool {
        !openOnly && days.isEmpty && time == .any && mode.isEmpty && level.isEmpty
    }

    var activeCount: Int {
        [openOnly, !days.isEmpty, time != .any, !mode.isEmpty, !level.isEmpty].filter { $0 }.count
    }
}

struct CatalogSearch: Sendable {
    var courses: [Course] = []
    var seats: [String: Seats] = [:]
    var total = 0
}

/// The term's catalog, held off the main actor. Downloading, decoding, indexing,
/// and every keystroke's search happen here so the planner never stutters.
actor CatalogIndex {
    enum Status: Sendable, Equatable {
        case idle, loading, ready, failed(String)
    }

    private struct Entry {
        let code: String
        let compact: String
        let subject: String
        let titleWords: [String]
        let peopleWords: [String]
        let hay: String
    }

    private let api: HonorumAPI
    private var key = ""
    /// The structure publication the index was built from.
    private var indexed = ""
    private var file: CatalogFile?
    private var entries: [Entry] = []
    private var seats: [String: Seats] = [:]
    private var courseOfSection: [String: Int] = [:]
    /// Course positions in code order, for browsing without a query.
    private var browseOrder: [Int] = []
    private static let log = Logger(subsystem: "app.honorum.ios", category: "catalog")
    private(set) var status = Status.idle
    private(set) var modes: [String] = []

    init(api: HonorumAPI = HonorumAPI()) { self.api = api }

    var isLoaded: Bool { file != nil }
    var courseCount: Int { file?.courses.count ?? 0 }

    // MARK: Loading

    /// Loads a term, reusing what is already indexed. Structure and seats are
    /// independent publications: the catalog becomes searchable as soon as the
    /// structure arrives, and seat counts fill in when they land. Seats are
    /// published far more often, so new counts alone leave the index standing.
    func load(inst: String, term: Term) async {
        let structureID = "\(inst)/\(term.id)/\(term.structureSnapshot ?? "")"
        let identity = "\(structureID)/\(term.seatsSnapshot ?? "")"
        if identity == key, status == .ready { return }
        key = identity
        guard let structureHash = term.structureSnapshot else {
            status = file == nil ? .failed("This term is not published yet.") : .ready
            return
        }

        if structureID == indexed, file != nil {
            status = .ready
        } else {
            status = .loading
            do {
                let structure = try await api.structure(
                    inst: inst, term: term.id, hash: structureHash)
                guard key == identity else { return }
                index(structure)
                indexed = structureID
                status = .ready
            } catch {
                guard key == identity else { return }
                if file == nil { status = .failed(message(for: error)) } else { status = .ready }
                return
            }
        }

        if let hash = term.seatsSnapshot {
            if let counts = try? await api.seats(inst: inst, term: term.id, hash: hash),
                key == identity
            {
                seats = counts.table()
            }
        }
    }

    /// Adopts an already-decoded publication, without going to the network.
    /// Previews and tests use it; the app reaches it through `load`.
    func adopt(_ structure: CatalogFile) {
        index(structure)
        status = .ready
    }

    private func index(_ structure: CatalogFile) {
        let began = ContinuousClock.now
        file = structure
        indexed = ""
        courseOfSection = [:]
        var modeNames = Set<String>()
        entries = structure.courses.enumerated().map { position, course in
            for section in course.sections {
                courseOfSection[section.id] = position
                if let mode = section.mode, !mode.isEmpty { modeNames.insert(mode) }
            }
            let code = course.code.lowercased()
            let people = Set(course.sections.flatMap { $0.teachers }).joined(separator: " ")
            return Entry(
                code: code,
                compact: code.replacingOccurrences(of: " ", with: ""),
                subject: course.subject.lowercased(),
                titleWords: Self.words(course.title),
                peopleWords: Self.words(people),
                hay: "\(code) \(course.title.lowercased()) \(people.lowercased())")
        }
        modes = modeNames.sorted()
        browseOrder = entries.indices.sorted { entries[$0].code < entries[$1].code }
        let elapsed = began.duration(to: .now)
        let milliseconds = Double(elapsed.components.seconds) * 1000
            + Double(elapsed.components.attoseconds) / 1e15
        Self.log.info(
            "Indexed \(structure.courses.count) courses in \(milliseconds, format: .fixed(precision: 0)) ms")
    }

    private func message(for error: Error) -> String {
        if case HonorumAPI.Failure.notFound = error { return "This term is not published yet." }
        if (error as? URLError) != nil { return "Classes need a connection to load." }
        return "Classes could not be loaded."
    }

    // MARK: Reading

    func publishedExams(inst: String, term: String) -> [String: [Exam]] {
        guard let file, file.inst == inst, file.term.id == term else { return [:] }
        return Dictionary(uniqueKeysWithValues: file.courses.flatMap { $0.sections }.map { ($0.id, $0.sittings) })
    }

    func snapshot() -> (status: Status, modes: [String]) { (status, modes) }

    func course(_ code: String) -> Course? {
        file?.courses.first { $0.code == code }
    }

    func result(for code: String) -> CatalogSearch? {
        guard let course = course(code) else { return nil }
        return CatalogSearch(courses: [course], seats: counts(for: [course]), total: 1)
    }

    func counts(ids: [String]) -> [String: Seats] {
        ids.reduce(into: [:]) { table, id in table[id] = seats[id] }
    }

    /// Replaces cached counts with a live reading for the sections on screen.
    func merge(live: [String: Seats]) {
        for (id, value) in live { seats[id] = value }
    }

    /// Ranked course results for a query, trimmed to sections that pass the
    /// filters. Scoring puts the course code and subject ahead of title words,
    /// which stay ahead of instructors, so "mis" finds MIS and not chemistry.
    func search(_ query: String, filters: CatalogFilters, limit: Int) -> CatalogSearch {
        guard let file else { return CatalogSearch() }
        let normalized = Self.normalize(query)

        // A narrowed query already knows its exact courses, sections and all.
        if !normalized.isEmpty, let narrowed = narrowToSection(normalized) {
            let trimmed = apply(filters, to: narrowed)
            let page = Array(trimmed.prefix(limit))
            return CatalogSearch(courses: page, seats: counts(for: page), total: trimmed.count)
        }

        // Otherwise rank positions and only materialize the page that is shown.
        let order: [Int]
        if normalized.isEmpty {
            order = browseOrder
        } else {
            let compact = normalized.replacingOccurrences(of: " ", with: "")
            let tokens = normalized.split(separator: " ").map(String.init)
            var hits: [(score: Int, position: Int)] = []
            hits.reserveCapacity(64)
            for (position, entry) in entries.enumerated() {
                let value = score(entry, normalized, compact, tokens)
                if value > 0 { hits.append((value, position)) }
            }
            hits.sort {
                $0.score != $1.score ? $0.score > $1.score : entries[$0.position].code < entries[$1.position].code
            }
            order = hits.map(\.position)
        }

        if filters.isDefault {
            let page = order.prefix(limit).map { file.courses[$0] }
            return CatalogSearch(courses: page, seats: counts(for: page), total: order.count)
        }

        var page: [Course] = []
        var total = 0
        for position in order {
            let course = file.courses[position]
            let sections = course.sections.filter { passes($0, filters, course) }
            guard !sections.isEmpty else { continue }
            total += 1
            if page.count < limit {
                var trimmed = course
                trimmed.sections = sections
                page.append(trimmed)
            }
        }
        return CatalogSearch(courses: page, seats: counts(for: page), total: total)
    }

    private func counts(for courses: [Course]) -> [String: Seats] {
        var table: [String: Seats] = [:]
        for course in courses {
            for section in course.sections where seats[section.id] != nil {
                table[section.id] = seats[section.id]
            }
        }
        return table
    }

    // MARK: Matching

    private static func normalize(_ query: String) -> String {
        var value = query.lowercased()
        for separator in ["-", "_", "/", ".", ":"] {
            value = value.replacingOccurrences(of: separator, with: " ")
        }
        return value.split(separator: " ").joined(separator: " ")
    }

    private static func words(_ text: String) -> [String] {
        text.lowercased().split { !$0.isLetter && !$0.isNumber }.map(String.init)
    }

    private func score(_ entry: Entry, _ query: String, _ compact: String, _ tokens: [String]) -> Int {
        var total = 0
        if entry.subject == query {
            total += 1000
        } else if entry.code.hasPrefix(query) || entry.compact.hasPrefix(compact) {
            total += 500 - entry.compact.count
        }
        for token in tokens {
            if entry.subject == token {
                total += 100
            } else if entry.compact.hasPrefix(token) {
                total += 80
            } else if entry.titleWords.contains(where: { $0.hasPrefix(token) }) {
                total += 40
            } else if entry.peopleWords.contains(where: { $0.hasPrefix(token) }) {
                total += 20
            } else if entry.hay.contains(token) {
                total += 1
            } else {
                return 0  // every token has to match somewhere
            }
        }
        return total
    }

    /// "B AD 1001 006" trims a named course to the sections whose number
    /// matches, and a bare five-digit id finds its own section.
    private func narrowToSection(_ query: String) -> [Course]? {
        guard let file else { return nil }
        let tokens = query.split(separator: " ").map(String.init)
        if tokens.count >= 2, let last = tokens.last {
            let base = tokens.dropLast().joined(separator: " ")
            let baseCompact = base.replacingOccurrences(of: " ", with: "")
            var out: [Course] = []
            for (position, entry) in entries.enumerated()
            where entry.compact == baseCompact || entry.code == base {
                var course = file.courses[position]
                course.sections = course.sections.filter { Self.numberMatches($0.num, last) }
                if !course.sections.isEmpty { out.append(course) }
            }
            if !out.isEmpty { return out }
        }
        if tokens.count == 1, query.count >= 4, query.allSatisfy(\.isNumber),
            let position = courseOfSection[query]
        {
            var course = file.courses[position]
            course.sections = course.sections.filter { $0.id == query }
            return [course]
        }
        return nil
    }

    /// Section numbers differ by school: match exactly, by prefix, or ignoring
    /// leading zeros when the student typed none.
    private static func numberMatches(_ number: String, _ token: String) -> Bool {
        let value = number.lowercased()
        if value == token || value.hasPrefix(token) { return true }
        guard !token.hasPrefix("0") else { return false }
        return String(value.drop { $0 == "0" }) == token
    }

    // MARK: Filters

    private func apply(_ filters: CatalogFilters, to courses: [Course]) -> [Course] {
        guard !filters.isDefault else { return courses }
        var out: [Course] = []
        out.reserveCapacity(courses.count)
        for course in courses {
            var trimmed = course
            trimmed.sections = course.sections.filter { passes($0, filters, course) }
            if !trimmed.sections.isEmpty { out.append(trimmed) }
        }
        return out
    }

    private func passes(_ section: CourseSection, _ filters: CatalogFilters, _ course: Course) -> Bool {
        if filters.openOnly, !(seats[section.id]?.isOpen ?? false) { return false }
        if !filters.days.isEmpty {
            let allowed = Set(filters.days)
            guard section.days.allSatisfy({ $0.days.allSatisfy(allowed.contains) }) else {
                return false
            }
        }
        if let window = filters.time.range {
            guard !section.days.isEmpty,
                section.days.allSatisfy({ window.contains($0.start) })
            else { return false }
        }
        if !filters.mode.isEmpty, section.mode != filters.mode { return false }
        if !filters.level.isEmpty {
            guard let digit = Self.levelDigit(course.code) else { return false }
            if filters.level == "grad" ? digit < 5 : String(digit) != filters.level { return false }
        }
        return true
    }

    /// The first digit of the course number: "CS 1213" is level 1.
    private static func levelDigit(_ code: String) -> Int? {
        guard let last = code.split(separator: " ").last else { return nil }
        return last.first { $0.isNumber }?.wholeNumberValue
    }
}
