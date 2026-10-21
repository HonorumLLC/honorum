import Foundation
import SwiftUI
import UIKit

/// Everything the interface reads. The saved library is the source of truth for
/// planning; the network only ever adds to it, and never blocks it.
@MainActor
@Observable
final class AppState {
    enum SyncStatus: Equatable, Sendable {
        case idle, syncing, synced, failed(String)
    }

    // Saved plan
    var library = Library()
    var syncState = SyncState()

    // Account
    private(set) var account: Account?
    var accountCanvas: CanvasConnection?
    var streak: AccountStreak?
    var activitySending = false
    /// Task documents moved in the last exchange, so the server's count of the
    /// streak may have too.
    var streakStale = false
    var accountCanvasRefreshing = false
    var canvasGeneration = 0
    var canvasCompletionSending: Set<String> = []
    var accountSyncJob: Task<Void, Never>?

    private(set) var token: String?
    private(set) var credentialsRead = false
    private(set) var syncStatus = SyncStatus.idle
    /// The last outcome, unchanged while an automatic exchange is in flight, so
    /// the interface can describe the account without flickering every minute.
    private(set) var settledSyncStatus = SyncStatus.idle
    private(set) var lastSynced: Date?
    private(set) var serverWatches: [ServerWatch] = []

    /// A failure the student should hear about, shown once by the root view.
    /// Automatic work stays quiet; actions the student asked for report here.
    var notice: UserNotice?
    /// Whether the saved plan has been read, so the first frame can wait for
    /// it rather than showing the welcome tour to a returning student.
    private(set) var loaded = false
    private var started = false
    /// Where a tapped notification wants the planner to go.
    var pendingRoute: PendingRoute?

    // Server configuration
    private(set) var config: AppConfig?
    private(set) var configError: String?
    private(set) var isRefreshing = false

    // Catalog
    let catalog = CatalogIndex()
    private(set) var catalogStatus = CatalogIndex.Status.idle
    private(set) var catalogModes: [String] = []

    // Planner selection
    var selectedDate = CivilDate.today(in: .current)
    /// Onboarding ended with the student asking for the class search.
    var editClassesOnOpen = false

    // Alerts and assignments
    private(set) var seatCounts: [String: Seats] = [:]
    private(set) var importStatus = ImportStatus.idle
    private(set) var canvasBase = ""
    private(set) var feeds: [String] = []
    /// Calendar feeds the account holds; the server imports them for every device.
    var accountFeeds: [AccountFeed] = []

    // Native push and live updates
    private(set) var apnsToken: String? = UserDefaults.standard.string(forKey: "apns-token")
    /// The owner and token the server last accepted, so registration happens once.
    var pushRegistered = ""
    var liveTask: Task<Void, Never>?
    var liveGeneration = 0

    let store = LibraryStore(directoryName: {
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("-uiTesting") { return "HonorumUITests" }
        #endif
        return "Honorum"
    }())
    /// A variable so the debug account fixture can point it nowhere.
    @ObservationIgnored var api = HonorumAPI()
    private var saveTask: Task<Void, Never>?
    private var syncTask: Task<Void, Never>?
    @ObservationIgnored private var closureCache: (id: String, index: [CivilDate: Closure])?
    @ObservationIgnored var termRecordCache: (key: String, record: TermRecord)?
    @ObservationIgnored private var courseResolverCache: TaskCourseResolver?
    @ObservationIgnored private var dueIndexCache: (tasks: [StudentTask], zone: TimeZone, index: [CivilDate: [StudentTask]])?

    var owner: String { account?.id ?? LibraryStore.guestOwner }
    var isSignedIn: Bool { account != nil && token != nil }

    // MARK: Derived

    var institution: Institution? {
        guard let id = library.inst else { return config?.institutions.first }
        return config?.institutions.first { $0.id == id } ?? config?.institutions.first
    }

    var term: Term? {
        guard let institution else { return nil }
        if let id = library.term, let match = institution.terms.first(where: { $0.id == id }) {
            return match
        }
        return Self.currentTerm(of: institution)
    }

    var timeZone: TimeZone { institution?.timeZone ?? .current }
    var clock: CampusClock { CampusClock(in: timeZone) }
    var today: CivilDate { clock.date }

    var closures: [CivilDate: Closure] {
        guard let institution else { return [:] }
        if let cached = closureCache, cached.id == institution.id { return cached.index }
        let index = institution.closureIndex()
        closureCache = (institution.id, index)
        return index
    }

    var schedule: Schedule {
        get { library.schedule }
        set { library.schedule = newValue; scheduleSave() }
    }

    /// Task-to-class matching, remembered across renders. A new resolver is
    /// built only when the term, its schedules, or the course map change.
    var taskCourses: TaskCourseResolver {
        let inputs = TaskCourseResolver.Inputs(
            inst: library.inst, term: library.term, sections: library.termSections,
            courseMap: library.courseMap)
        if let cached = courseResolverCache, cached.inputs == inputs { return cached }
        let resolver = TaskCourseResolver(inputs: inputs)
        courseResolverCache = resolver
        return resolver
    }

    /// Tasks due on a campus date, earliest first. The schedule asks for several
    /// dates on every frame of a day swipe, so the answer is indexed once per
    /// change to the tasks rather than parsed from each deadline every time.
    func tasks(on date: CivilDate) -> [StudentTask] {
        let zone = timeZone
        if let cached = dueIndexCache, cached.zone == zone, cached.tasks == library.tasks {
            return cached.index[date] ?? []
        }
        var index: [CivilDate: [StudentTask]] = [:]
        for task in library.tasks {
            guard let due = task.dueDate else { continue }
            index[CivilDate(due, in: zone), default: []].append(task)
        }
        for key in index.keys { index[key]?.sort { $0.due < $1.due } }
        dueIndexCache = (library.tasks, zone, index)
        return index[date] ?? []
    }

    func occurrences(on date: CivilDate) -> [Occurrence] {
        Occurrences.onDay(date, schedule: library.schedule, term: term, closures: closures)
    }

    /// Sections in the current schedule that clash with another, by section id.
    var conflicts: Set<String> {
        var found: Set<String> = []
        let sections = library.schedule.sections
        for a in sections.indices {
            for b in sections.indices where b > a {
                if Self.clash(sections[a], sections[b]) {
                    found.insert(sections[a].id)
                    found.insert(sections[b].id)
                }
            }
        }
        return found
    }

    private static func clash(_ a: SavedSection, _ b: SavedSection) -> Bool {
        for first in a.meetings {
            for second in b.meetings
            where first.start < second.end && second.start < first.end
                && first.days.contains(where: second.days.contains) {
                return true
            }
        }
        return false
    }

    // MARK: Lifecycle

    /// Reads the saved library before the first frame, then refreshes in the
    /// background. Planning works whether or not the network answers, and the
    /// Keychain read never sits in front of the first screen.
    func start() async {
        // One shared state, however many scenes an iPad opens.
        guard !started else { return }
        started = true
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("-uiTesting") {
            UserDefaults.standard.set(false, forKey: "tasksAtDueTime")
            // A sort one test chose must not reorder the next test's list.
            UserDefaults.standard.removeObject(forKey: "activeTaskSort")
            UserDefaults.standard.removeObject(forKey: "activeTaskSortReversed")
            if ProcessInfo.processInfo.arguments.contains("-accountFixture") {
                // A signed-in account for screenshots, whose requests go
                // nowhere and fail quietly rather than signing it out.
                api.origin = URL(string: "http://127.0.0.1:9")!
                setAccount(UITestFixture.account, token: "fixture")
                setSyncStatus(.synced, at: Date())
            }
            if ProcessInfo.processInfo.arguments.contains("-streakFixture") {
                let today = CivilDate.today(in: TimeZone(identifier: "America/Chicago")!)
                streak = AccountStreak(
                    current: 7, longest: 12, today: today.iso, lastDay: today.iso, timezone: "America/Chicago",
                    overdue: 0,
                    days: (0..<28).map { offset in
                        AccountStreak.Day(day: today.adding(days: offset - 27).iso, state: offset >= 21 || offset == 12 ? "clean" : offset > 8 ? "missed" : "clean")
                    })
            }
            config = UITestFixture.config
            library = UITestFixture.library
            selectedDate = today
            loaded = true
            return
        }
        #endif
        // Everything the first frame draws from is read before it: the last
        // configuration (school, term, time zone, closures), the saved plan,
        // the session in the Keychain, and the streak the server last
        // reported. Nothing then arrives to change the screen a moment later.
        if let cached = Self.cachedConfig() { config = cached }
        let saved = UserDefaults.standard.string(forKey: "activeOwner") ?? LibraryStore.guestOwner
        let envelope = await store.load(owner: saved)
        library = envelope.library
        syncState = envelope.sync
        if !library.onboarded && library.holdsPlans {
            // Ownership is restored below; do not enqueue a save to the guest namespace here.
            library.onboarded = true
        }
        let session = await readCredentials(expecting: saved)
        selectedDate = today
        loaded = true

        if let session { Task { await resumeSession(token: session.token, id: session.id) } }
        await refresh()
        await Reminders.shared.rebuild(from: self)
        await checkWatchedSeats()
    }

    /// Credentials live in the Keychain, which is read off the first frame.
    /// The Keychain's part of launch: quick, and done before the first frame,
    /// so the Account tab and the streak open in their signed-in state.
    private func readCredentials(expecting saved: String) async -> (token: String, id: String)? {
        let found = await Task.detached {
            (
                token: Keychain.read(.sessionToken), account: Keychain.read(.accountID),
                canvas: Keychain.read(.canvasBase) ?? "", feeds: Keychain.feeds
            )
        }.value
        canvasBase = found.canvas
        feeds = found.feeds
        credentialsRead = true
        guard let sessionToken = found.token, let id = found.account, id == saved else { return nil }
        token = sessionToken
        account = Account(id: id, phone: "", device: "")
        if let data = UserDefaults.standard.data(forKey: "canvas-connection:" + id) {
            accountCanvas = try? JSONDecoder().decode(CanvasConnection.self, from: data)
        }
        streak = Self.cachedStreak(for: id)
        return (sessionToken, id)
    }

    /// The network's part: the exchange, live updates, push, the account, the
    /// streak, and assignments, after the planner is on screen.
    private func resumeSession(token sessionToken: String, id: String) async {
        await syncNow()
        guard owner == id, token == sessionToken else { return }
        startLiveUpdates()
        requestRemoteNotifications()
        await sendPushRegistration()
        await refreshAccount()
        await recordForegroundActivity()
        await refreshAssignments()
    }

    // MARK: Native push

    /// Asks iOS for a device token once the server can use one. The token
    /// arrives in the app delegate and is registered with the account.
    func requestRemoteNotifications() {
        guard config?.apns == true, isSignedIn else { return }
        UIApplication.shared.registerForRemoteNotifications()
    }

    func registerPush(token deviceToken: String) {
        apnsToken = deviceToken
        UserDefaults.standard.set(deviceToken, forKey: "apns-token")
        Task { await sendPushRegistration() }
    }

    func sendPushRegistration() async {
        guard let session = token, isSignedIn, let apnsToken, config?.apns == true else { return }
        let registration = owner + ":" + apnsToken
        guard pushRegistered != registration else { return }
        do {
            try await api.registerPush(apns: apnsToken, session: session)
            guard token == session else { return }
            pushRegistered = registration
            await Reminders.shared.rebuild(from: self)
        } catch {}
    }

    /// Once this phone is registered, the server sends the account's reminders
    /// and seat alerts itself; scheduling them locally too would notify twice.
    var serverDeliversNotifications: Bool { isSignedIn && !pushRegistered.isEmpty }

    func refresh() async {
        guard !isRefreshing else { return }
        isRefreshing = true
        defer { isRefreshing = false }
        do {
            let fetched = try await api.configuration()
            config = fetched
            configError = nil
            Self.cache(config: fetched)
            adoptDefaultsIfNeeded()
            requestRemoteNotifications()
            await sendPushRegistration()
        } catch {
            configError = (error as? LocalizedError)?.errorDescription
                ?? "Honorum could not reach the server."
        }
    }

    private func adoptDefaultsIfNeeded() {
        guard let institution else { return }
        if library.inst == nil {
            library.inst = institution.id
            scheduleSave()
        }
        if library.term == nil, let term = Self.currentTerm(of: institution) {
            library.term = term.id
            scheduleSave()
        }
        ensureSchedule()
        selectedDate = today
    }

    /// Every term the student opens gets a schedule to plan in.
    func ensureSchedule() {
        guard let inst = library.inst, let term = library.term else { return }
        if library.termSchedules.isEmpty {
            var created = Schedule(name: "Schedule 1", inst: inst, term: term, primary: true)
            created.id = UUID().uuidString.lowercased()
            library.schedules.append(created)
            library.current = created.id
            scheduleSave()
        } else if !library.termSchedules.contains(where: { $0.id == library.current }) {
            library.current = (library.termSchedules.first { $0.primary }
                ?? library.termSchedules[0]).id
            scheduleSave()
        }
    }

    /// The term that contains today, otherwise the next one to start.
    private static func currentTerm(of institution: Institution) -> Term? {
        let today = CivilDate.today(in: institution.timeZone)
        let containing = institution.terms.first { term in
            guard let start = CivilDate(term.start ?? ""), let end = CivilDate(term.end ?? "")
            else { return false }
            return start <= today && today <= end
        }
        if let containing { return containing }
        let upcoming = institution.terms
            .compactMap { term -> (CivilDate, Term)? in
                guard let start = CivilDate(term.start ?? ""), start > today else { return nil }
                return (start, term)
            }
            .min { $0.0 < $1.0 }
        return upcoming?.1 ?? institution.terms.first
    }

    // MARK: Catalog

    /// Identifies the term the catalog should hold. Changes when configuration
    /// arrives and when the student picks another school or term.
    var catalogKey: String? {
        guard let institution, let term else { return nil }
        return "\(institution.id)/\(term.id)/\(term.structureSnapshot ?? "")/\(term.seatsSnapshot ?? "")"
    }

    func loadCatalog() async {
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("-uiTesting") {
            if ProcessInfo.processInfo.arguments.contains("-catalogFixture"), catalogStatus != .ready {
                await catalog.adopt(UITestFixture.catalog)
                let snapshot = await catalog.snapshot()
                catalogStatus = snapshot.status
                catalogModes = snapshot.modes
            }
            return
        }
        #endif
        guard let institution, let term else { return }
        await catalog.load(inst: institution.id, term: term)
        let snapshot = await catalog.snapshot()
        catalogStatus = snapshot.status
        catalogModes = snapshot.modes
        let exams = await catalog.publishedExams(inst: institution.id, term: term.id)
        guard self.institution?.id == institution.id, self.term?.id == term.id else { return }
        var changed = false
        for i in library.schedules.indices where library.schedules[i].inst == institution.id && library.schedules[i].term == term.id {
            for j in library.schedules[i].sections.indices {
                let section = library.schedules[i].sections[j]
                if let published = exams[section.id], published != section.exams {
                    library.schedules[i].sections[j].exams = published
                    changed = true
                }
            }
        }
        if changed {
            scheduleSave()
            await Reminders.shared.rebuild(from: self)
        }
    }

    func search(_ query: String, filters: CatalogFilters, limit: Int) async -> CatalogSearch {
        await catalog.search(query, filters: filters, limit: limit)
    }

    /// Refreshes seat counts for the sections on screen, then reports them back.
    @discardableResult
    func liveSeats(for ids: [String]) async -> [String: Seats] {
        guard let institution, let term, !ids.isEmpty else { return [:] }
        guard let counts = try? await api.liveSeats(inst: institution.id, term: term.id, ids: ids)
        else { return [:] }
        await catalog.merge(live: counts)
        seatCounts.merge(counts) { _, new in new }
        return counts
    }

    // MARK: Editing the plan

    func isSaved(_ sectionID: String) -> Bool {
        library.schedule.sections.contains { $0.id == sectionID }
    }

    func add(_ section: CourseSection, from course: Course, seats: Seats? = nil) {
        var plan = library.schedule
        guard !plan.sections.contains(where: { $0.id == section.id }) else { return }
        plan.sections.append(
            SavedSection(
                section: section, course: course, catalogURL: catalogSourceURL, seats: seats))
        plan.sections.sort { $0.label < $1.label }
        library.schedule = plan
        scheduleSave()
    }

    private var catalogSourceURL: String { institution?.lms?.url ?? "" }

    func remove(sectionID: String) {
        var plan = library.schedule
        plan.sections.removeAll { $0.id == sectionID }
        plan.removed[sectionID] = nil
        let remaining = Set(plan.sections.map(\.course))
        plan.colors = plan.colors.filter { remaining.contains($0.key) }
        library.schedule = plan
        scheduleSave()
    }

    /// Takes one meeting off the plan without dropping the class.
    func skip(itemID: String, on date: CivilDate) {
        var plan = library.schedule
        var dates = Set(plan.removed[itemID] ?? [])
        dates.insert(date.iso)
        plan.removed[itemID] = dates.sorted()
        library.schedule = plan
        scheduleSave()
    }

    /// Puts a skipped meeting back on the week.
    func restore(itemID: String, on date: CivilDate) {
        var plan = library.schedule
        let dates = (plan.removed[itemID] ?? []).filter { $0 != date.iso }
        plan.removed[itemID] = dates.isEmpty ? nil : dates
        library.schedule = plan
        scheduleSave()
    }

    /// Items whose meeting on this date was taken off the plan.
    func skipped(on date: CivilDate) -> [String] {
        library.schedule.removed.filter { $0.value.contains(date.iso) }.map(\.key)
    }

    func save(event: PersonalEvent) {
        var plan = library.schedule
        if let index = plan.events.firstIndex(where: { $0.id == event.id }) {
            plan.events[index] = event
        } else {
            plan.events.append(event)
        }
        library.schedule = plan
        scheduleSave()
    }

    func removeEvent(id: String) {
        var plan = library.schedule
        plan.events.removeAll { $0.id == id }
        plan.removed[id] = nil
        library.schedule = plan
        scheduleSave()
    }

    func event(id: String) -> PersonalEvent? { library.schedule.events.first { $0.id == id } }
    func section(id: String) -> SavedSection? { library.schedule.sections.first { $0.id == id } }

    // MARK: Schedules

    func newSchedule(named name: String, copying source: Schedule? = nil) {
        guard let inst = library.inst, let term = library.term else { return }
        var created = Schedule(
            name: name, inst: inst, term: term, primary: library.termSchedules.isEmpty)
        created.id = UUID().uuidString.lowercased()
        if let source {
            created.sections = source.sections
            created.colors = source.colors
            created.events = source.events.map { event in
                var copy = event
                copy.id = UUID().uuidString.lowercased()
                return copy
            }
        }
        library.schedules.append(created)
        library.current = created.id
        scheduleSave()
    }

    func rename(scheduleID: String, to name: String) {
        guard let index = library.schedules.firstIndex(where: { $0.id == scheduleID }) else { return }
        library.schedules[index].name = name
        scheduleSave()
    }

    func makePrimary(scheduleID: String) {
        for index in library.schedules.indices
        where library.schedules[index].inst == library.inst
            && library.schedules[index].term == library.term
        {
            library.schedules[index].primary = library.schedules[index].id == scheduleID
        }
        scheduleSave()
    }

    func deleteSchedule(id: String) {
        library.schedules.removeAll { $0.id == id }
        if library.current == id { library.current = nil }
        ensureSchedule()
        scheduleSave()
    }

    func open(scheduleID: String) {
        library.current = scheduleID
        scheduleSave()
    }

    // MARK: Tasks

    func save(task: StudentTask) {
        var task = task
        if let index = library.tasks.firstIndex(where: { $0.id == task.id }) {
            // An editor opened before the last completion keeps its recorded date.
            task.completedAt = library.tasks[index].completedAt
            task.recordCompletion(previousDone: library.tasks[index].done)
            if task.source == .canvas && task.done != library.tasks[index].done {
                if accountCanvas?.connected == true { task.pendingCanvasDone = UUID().uuidString.lowercased() }
                let changed = task
                Task { await reportCanvasCompletion(of: changed) }
            }
            library.tasks[index] = task
        } else {
            task.recordCompletion(previousDone: false)
            library.tasks.append(task)
        }
        scheduleSave()
        Task { await Reminders.shared.rebuild(from: self) }
    }

    func toggle(taskID: String) {
        guard let index = library.tasks.firstIndex(where: { $0.id == taskID }) else { return }
        let wasDone = library.tasks[index].done
        library.tasks[index].done.toggle()
        library.tasks[index].recordCompletion(previousDone: wasDone)
        if library.tasks[index].source == .canvas && accountCanvas?.connected == true {
            library.tasks[index].pendingCanvasDone = UUID().uuidString.lowercased()
        }
        let task = library.tasks[index]
        scheduleSave()
        if task.source == .canvas { Task { await reportCanvasCompletion(of: task) } }
    }

    func removeTask(id: String) {
        if let task = library.tasks.first(where: { $0.id == id }) { forgetTermTask(task) }
        library.tasks.removeAll { $0.id == id }
        scheduleSave()
    }

    // MARK: Preferences

    func choose(institution id: String) {
        guard library.inst != id else { return }
        library.inst = id
        library.term = config?.institutions.first { $0.id == id }.flatMap(Self.currentTerm)?.id
        library.current = nil
        ensureSchedule()
        selectedDate = today
        scheduleSave()
    }

    func choose(term id: String) {
        guard library.term != id else { return }
        library.term = id
        library.current = nil
        ensureSchedule()
        selectedDate = today
        scheduleSave()
    }

    func finishOnboarding() {
        library.onboarded = true
        ensureSchedule()
        scheduleSave()
    }

    /// Turns class reminders on or off. Returns false when iOS has refused
    /// notifications, so the caller can say why the switch stayed off.
    @discardableResult
    func setReminders(_ on: Bool) async -> Bool {
        if on, !(await Reminders.shared.requestPermission()) { return false }
        library.reminders = on
        scheduleSave()
        await Reminders.shared.rebuild(from: self)
        return true
    }

    /// Follows a tapped notification: a class reminder opens its day, a task
    /// reminder opens the task, and a seat alert opens the Account tab, where
    /// the alerts are.
    func route(notification identifier: String) {
        let id = identifier.replacingOccurrences(of: "honorum.reminder.", with: "")
        let parts = id.split(separator: ".", maxSplits: 2).map(String.init)
        if parts.first == "class", parts.count == 3, let date = CivilDate(parts[1]) {
            pendingRoute = .day(date)
        } else if parts.first == "task", parts.count == 2 {
            pendingRoute = .task(parts[1])
        } else if identifier.hasPrefix("honorum.seat.") {
            pendingRoute = .alerts
        }
    }

    // MARK: Launch caches

    /// The last configuration and streak, kept in the app's defaults so the
    /// next launch draws them before the server answers.
    private static let configKey = "config"
    private static func streakKey(for owner: String) -> String { "streak:" + owner }

    private static func cachedConfig() -> AppConfig? {
        guard let data = UserDefaults.standard.data(forKey: configKey) else { return nil }
        return try? JSONDecoder().decode(AppConfig.self, from: data)
    }

    private static func cache(config: AppConfig) {
        if let data = try? JSONEncoder().encode(config) { UserDefaults.standard.set(data, forKey: configKey) }
    }

    static func cachedStreak(for owner: String) -> AccountStreak? {
        guard let data = UserDefaults.standard.data(forKey: streakKey(for: owner)) else { return nil }
        return try? JSONDecoder().decode(AccountStreak.self, from: data)
    }

    func cache(streak: AccountStreak, for owner: String) {
        if let data = try? JSONEncoder().encode(streak) {
            UserDefaults.standard.set(data, forKey: Self.streakKey(for: owner))
        }
    }

    static func forgetStreak(for owner: String) {
        UserDefaults.standard.removeObject(forKey: streakKey(for: owner))
    }

    /// Reports a failed action by its outcome, with the server's wording.
    func report(_ title: String, _ error: Error) {
        notice = UserNotice(title: title, message: (error as? LocalizedError)?.errorDescription ?? error.localizedDescription)
    }

    // MARK: Saving

    /// Coalesces the writes a burst of edits produces into one atomic save, and
    /// lets the account catch up shortly afterwards.
    func scheduleSave() {
        saveTask?.cancel()
        let snapshot = Envelope(library: library, sync: syncState)
        let owner = owner
        saveTask = Task { [store] in
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled else { return }
            await store.save(owner: owner, snapshot)
        }
        syncSoon()
    }

    func saveNow() {
        saveTask?.cancel()
        let snapshot = Envelope(library: library, sync: syncState)
        let owner = owner
        Task { [store] in await store.save(owner: owner, snapshot) }
    }

    /// Saves and returns once the envelope is on disk, for the few callers
    /// that must not move on before it is.
    func saveAndWait() async {
        saveTask?.cancel()
        await store.save(owner: owner, Envelope(library: library, sync: syncState))
    }

    /// Uploads shortly after an edit settles, so a burst of typing produces one
    /// exchange rather than one per keystroke.
    func syncSoon() {
        guard isSignedIn else { return }
        syncTask?.cancel()
        syncTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(2))
            guard !Task.isCancelled else { return }
            await self?.syncNow()
        }
    }

    // MARK: Reachable from the extensions

    func setAccount(_ value: Account?, token newToken: String?) {
        if account?.id != value?.id {
            canvasGeneration += 1
            accountCanvas = nil
            accountFeeds = []
            streak = nil
            pushRegistered = ""
        }
        account = value
        token = newToken
    }

    func setSyncStatus(_ value: SyncStatus, at date: Date? = nil) {
        syncStatus = value
        if value != .syncing { settledSyncStatus = value }
        if let date { lastSynced = date }
    }

    func setServerWatches(_ value: [ServerWatch]) { serverWatches = value }
    func setImportStatus(_ value: ImportStatus) { importStatus = value }
    func setCanvasBase(_ value: String) { canvasBase = value }
    func setFeeds(_ value: [String]) { feeds = value }
}

/// Something that went wrong with what the student just asked for.
struct UserNotice: Identifiable, Equatable {
    let id = UUID()
    let title: String
    let message: String
}

/// Where a notification sends the planner.
enum PendingRoute: Equatable {
    case day(CivilDate)
    case task(String)
    case alerts
}

#if DEBUG
extension AppState {
    /// A planner holding the UI test fixture, for previews.
    static func preview() -> AppState {
        fixture(config: UITestFixture.config, library: UITestFixture.library)
    }

    /// A planner holding the given configuration and plan, without a network.
    static func fixture(config: AppConfig, library: Library) -> AppState {
        let state = AppState()
        state.started = true
        state.config = config
        state.library = library
        state.selectedDate = state.today
        state.loaded = true
        return state
    }
}
#endif
