import Foundation

enum ImportStatus: Equatable, Sendable {
    case idle
    case importing(String)
    case failed(String)
    case finished(Date)
}

/// Account Canvas imports publish through document sync. Guest Canvas tokens
/// and calendar feed URLs stay in Keychain and pass through the stateless relay.
extension AppState {
    var canvasConnected: Bool { accountCanvas?.connected == true || (!canvasBase.isEmpty && Keychain.read(.canvasToken) != nil) }
    var assignmentsConnected: Bool { canvasConnected || !feeds.isEmpty || !accountFeeds.isEmpty }

    // MARK: Connecting

    func connectCanvas(base: String, token: String) async throws {
        canvasGeneration += 1
        let generation = canvasGeneration
        let address = Self.normalize(base)
        if let session = self.token {
            let accountID = owner
            await syncNow()
            guard owner == accountID, generation == canvasGeneration, syncStatus == .synced else { throw HonorumAPI.Failure.offline }
            let connection = try await api.connectAccountCanvas(base: address, credential: token, token: session)
            guard owner == accountID, generation == canvasGeneration else { return }
            canvasGeneration += 1
            saveAccountCanvas(connection)
            Keychain.remove(.canvasToken)
            Keychain.remove(.canvasBase)
            setCanvasBase("")
            await syncNow()
            return
        }
        _ = try await api.canvasTasks(base: address, token: token)
        guard generation == canvasGeneration else { return }
        Keychain.write(.canvasBase, address)
        Keychain.write(.canvasToken, token)
        setCanvasBase(address)
        await refreshAssignments(force: true)
    }

    func disconnectCanvas() async {
        canvasGeneration += 1
        let generation = canvasGeneration
        if let session = token, accountCanvas?.connected == true {
            let accountID = owner
            do {
                let connection = try await api.disconnectAccountCanvas(token: session)
                guard owner == accountID, generation == canvasGeneration else { return }
                canvasGeneration += 1
                saveAccountCanvas(connection)
            } catch { setImportStatus(.failed("Canvas could not be disconnected. Try again.")) }
            return
        }
        Keychain.remove(.canvasBase)
        Keychain.remove(.canvasToken)
        setCanvasBase("")
        library.tasks.removeAll { $0.source == .canvas }
        scheduleSave()
    }

    /// Signed in, the link is saved encrypted with the account and the server
    /// imports it for every device. As a guest it stays in the Keychain.
    func addFeed(url: String) async throws {
        let address = Self.normalize(url)
        if let session = token, isSignedIn {
            let accountID = owner
            let list = try await api.addAccountFeed(url: address, label: "", token: session)
            guard owner == accountID else { return }
            accountFeeds = list.feeds
            await syncNow()
            await Reminders.shared.rebuild(from: self)
            return
        }
        _ = try await api.calendarFeed(url: address)
        var updated = feeds
        guard !updated.contains(address) else { return }
        updated.append(address)
        Keychain.feeds = updated
        setFeeds(updated)
        await refreshAssignments(force: true)
    }

    func removeAccountFeed(id: String) async {
        guard let session = token else { return }
        let accountID = owner
        do {
            let list = try await api.removeAccountFeed(id: id, token: session)
            guard owner == accountID else { return }
            accountFeeds = list.feeds
            await syncNow()
        } catch {
            setImportStatus(.failed("The calendar could not be removed. Try again."))
        }
    }

    func removeFeed(url: String) {
        let updated = feeds.filter { $0 != url }
        Keychain.feeds = updated
        setFeeds(updated)
        if updated.isEmpty {
            reconcile([], source: .feed)
        } else {
            Task { await refreshAssignments(force: true) }
        }
    }

    // MARK: Classes from Canvas

    /// The student's Canvas courses matched to the catalog by the server,
    /// read with the account's saved token or with this device's own.
    func findCanvasClasses() async throws -> FoundClasses {
        guard let institution else { throw HonorumAPI.Failure.status(400, "Choose your school first.") }
        let termID = term?.id ?? ""
        if let session = token, accountCanvas?.connected == true {
            return try await api.accountCanvasClasses(inst: institution.id, term: termID, token: session)
        }
        guard !canvasBase.isEmpty, let canvasToken = Keychain.read(.canvasToken) else {
            throw HonorumAPI.Failure.status(400, "Connect Canvas first.")
        }
        return try await api.canvasClasses(base: canvasBase, token: canvasToken, inst: institution.id, term: termID)
    }

    /// Adds the picked sections to the found term's schedule and matches each
    /// Canvas course to its class, so its assignments take the class's color.
    func addFound(_ picks: [(course: FoundClasses.Source, code: String, ids: [String])], from found: FoundClasses) {
        if !found.term.isEmpty, found.term != library.term,
           institution?.terms.contains(where: { $0.id == found.term }) == true {
            choose(term: found.term)
        }
        var plan = library.schedule
        for pick in picks {
            for id in pick.ids where !plan.sections.contains(where: { $0.id == id }) {
                guard let body = found.sections[id] else { continue }
                plan.sections.append(SavedSection(body))
            }
        }
        plan.sections.sort { $0.label < $1.label }
        library.schedule = plan
        for pick in picks {
            let connection = library.tasks.first { $0.courseID == pick.course.id }?.extras["canvasConnection"]?.stringValue ?? ""
            map(lmsCourse: pick.course.id, to: pick.code, connection: connection)
        }
        scheduleSave()
    }

    private static func normalize(_ address: String) -> String {
        var text = address.trimmingCharacters(in: .whitespacesAndNewlines)
        if !text.lowercased().hasPrefix("http") { text = "https://" + text }
        while text.hasSuffix("/") { text.removeLast() }
        return text
    }

    // MARK: Importing

    /// Fetches every connected source. A source that fails leaves its existing
    /// tasks alone: an incomplete import must never delete saved work.
    func refreshAssignments(force: Bool = false) async {
        #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("-uiTesting"),
           ProcessInfo.processInfo.arguments.contains("-refreshFixture") {
            guard force else { return }
            setImportStatus(.importing("Canvas"))
            try? await Task.sleep(for: .seconds(5))
            setImportStatus(.finished(Date()))
            return
        }
        #endif
        guard !accountCanvasRefreshing else { return }
        // The minute timer only checks freshness. Sources that succeeded within
        // the last five minutes are left alone unless the student pulls to
        // refresh, so an idle app is not exchanging documents every minute.
        if !force, case .finished(let when) = importStatus, Date().timeIntervalSince(when) < 5 * 60 {
            return
        }
        accountCanvasRefreshing = true
        defer { accountCanvasRefreshing = false }
        var problems: [String] = []
        var accountFailed = false
        let accountID = owner
        let generation = canvasGeneration
        if let session = token {
            do {
                let connection = try await api.accountCanvas(token: session)
                guard owner == accountID, generation == canvasGeneration else { return }
                saveAccountCanvas(connection)
                if let list = try? await api.accountFeeds(token: session) {
                    guard owner == accountID, generation == canvasGeneration else { return }
                    accountFeeds = list.feeds
                }
                if connection.connected || !accountFeeds.isEmpty {
                    setImportStatus(.importing(connection.connected ? "Canvas" : "Calendars"))
                    await syncNow()
                    guard owner == accountID, generation == canvasGeneration else { return }
                    guard syncStatus == .synced else { throw HonorumAPI.Failure.offline }
                    if connection.connected {
                        await retryPendingCanvasCompletions()
                        let fresh = try await api.refreshAccountCanvas(force: force, token: session)
                        guard owner == accountID, generation == canvasGeneration else { return }
                        saveAccountCanvas(fresh)
                    }
                    if !accountFeeds.isEmpty {
                        setImportStatus(.importing("Calendars"))
                        let list = try await api.refreshAccountFeeds(force: force, token: session)
                        guard owner == accountID, generation == canvasGeneration else { return }
                        accountFeeds = list.feeds
                    }
                    await syncNow()
                    guard owner == accountID, generation == canvasGeneration else { return }
                    setImportStatus(.finished(Date()))
                }
            } catch {
                guard owner == accountID, generation == canvasGeneration else { return }
                accountFailed = true
                problems.append((error as? LocalizedError)?.errorDescription ?? "Canvas could not be reached.")
                setImportStatus(.failed(problems[0]))
            }
        }
        guard assignmentsConnected else { return }
        if case .importing = importStatus { return }
        if accountCanvas?.connected != true, !force, case .finished(let when) = importStatus,
            Date().timeIntervalSince(when) < 5 * 60
        {
            return
        }

        if !accountFailed, accountCanvas?.connected != true, canvasConnected, let token = Keychain.read(.canvasToken) {
            setImportStatus(.importing("Canvas"))
            do {
                let imported = try await api.canvasTasks(base: canvasBase, token: token)
                guard owner == accountID, generation == canvasGeneration else { return }
                reconcile(imported, source: .canvas)
                await retryPendingCanvasCompletions()
            } catch {
                problems.append(
                    (error as? LocalizedError)?.errorDescription ?? "Canvas could not be reached.")
            }
        }

        if !feeds.isEmpty {
            setImportStatus(.importing("Calendars"))
            var collected: [ImportedTask] = []
            var complete = true
            for feed in feeds {
                do {
                    let text = try await api.calendarFeed(url: feed)
                    collected += ICalendar.tasks(from: text, feed: feed, timeZone: timeZone)
                } catch {
                    complete = false
                    problems.append(
                        (error as? LocalizedError)?.errorDescription
                            ?? "A calendar feed could not be read.")
                }
            }
            // A collection is replaced only when all of its feeds succeeded.
            guard owner == accountID, generation == canvasGeneration else { return }
            if complete { reconcile(collected, source: .feed) }
        }

        setImportStatus(problems.isEmpty ? .finished(Date()) : .failed(problems[0]))
        await Reminders.shared.rebuild(from: self)
    }

    /// Matches by upstream id, keeps local completion, class and due date
    /// overrides, and drops open tasks that disappeared upstream. Finished ones
    /// stay: a source looks back only a week or two.
    func reconcile(_ imported: [ImportedTask], source: StudentTask.Source) {
        var upstream = Dictionary(imported.map { ($0.uid, $0) }, uniquingKeysWith: { first, _ in first })
        var next: [StudentTask] = []
        next.reserveCapacity(library.tasks.count + imported.count)

        for var task in library.tasks {
            guard task.source == source, task.extras["canvasConnection"]?.stringValue?.isEmpty != false else {
                next.append(task)
                continue
            }
            guard let update = upstream.removeValue(forKey: task.uid) else {
                if task.done { next.append(task) }
                continue
            }
            task.title = update.title
            task.due = keptDue(task, upstream: update.due)
            task.importedDue = update.due
            task.url = update.url ?? ""
            task.overrideID = update.overrideId ?? ""
            task.moduleItem = update.moduleItem ?? ""
            task.courseID = update.courseId ?? ""
            task.courseCode = update.courseCode ?? ""
            if !task.courseEdited { task.course = matchedCourse(for: update) }
            // A tick that has not reached Canvas yet outranks what Canvas says.
            if task.pendingDone == nil {
                if task.done != update.done { task.completedAt = nil }
                task.done = update.done
            }
            next.append(task)
        }

        for update in imported where upstream[update.uid] != nil {
            var task = StudentTask()
            task.id = UUID().uuidString.lowercased()
            task.source = source
            task.uid = update.uid
            task.title = update.title
            task.due = update.due
            task.importedDue = update.due
            task.done = update.done
            task.url = update.url ?? ""
            task.overrideID = update.overrideId ?? ""
            task.moduleItem = update.moduleItem ?? ""
            task.courseID = update.courseId ?? ""
            task.courseCode = update.courseCode ?? ""
            task.course = matchedCourse(for: update)
            next.append(task)
            upstream[update.uid] = nil
        }

        library.tasks = next
        scheduleSave()
    }

    /// A due date the student changed stays until the source moves its own. A
    /// task imported before the source's date was recorded counts as unchanged.
    private func keptDue(_ task: StudentTask, upstream: String) -> String {
        let base = task.importedDue ?? task.due
        return !sameInstant(task.due, base) && sameInstant(upstream, base) ? task.due : upstream
    }

    private func sameInstant(_ a: String, _ b: String) -> Bool {
        guard a != b else { return true }
        guard let x = ISO8601.date(from: a) else { return false }
        return x == ISO8601.date(from: b)
    }

    /// The class this assignment belongs to: the student's own mapping first,
    /// then the code the provider reported, then its display name.
    private func matchedCourse(for update: ImportedTask) -> String {
        if let id = update.courseId, let mapped = library.courseMap[id], !mapped.isEmpty {
            return mapped
        }
        let codes = Set(library.schedule.sections.map(\.course))
        if let reported = update.courseCode {
            let compact = reported.uppercased().filter { !$0.isWhitespace }
            if let match = codes.first(where: {
                compact.hasPrefix($0.uppercased().filter { !$0.isWhitespace })
            }) {
                return match
            }
        }
        return update.course
    }

    /// Lets the student say which class an imported course belongs to once.
    /// The choice belongs to the connection that supplied the course, so the
    /// same upstream id on another Canvas site never inherits it.
    func map(lmsCourse id: String, to code: String, connection: String = "") {
        let key = connection.isEmpty ? id : "\(connection):\(id)"
        if code.isEmpty {
            library.courseMap[key] = nil
            library.courseMap[id] = nil
        } else {
            library.courseMap[key] = code
        }
        for index in library.tasks.indices
        where library.tasks[index].courseID == id && !library.tasks[index].courseEdited
            && (connection.isEmpty || library.tasks[index].extras["canvasConnection"]?.stringValue == connection)
        {
            library.tasks[index].course = code
        }
        scheduleSave()
    }

    /// The class a mapping names for an imported course, on its connection first.
    func mappedCourse(for courseID: String, connection: String) -> String {
        if !connection.isEmpty, let scoped = library.courseMap["\(connection):\(courseID)"], !scoped.isEmpty {
            return scoped
        }
        return library.courseMap[courseID] ?? ""
    }

    /// The imported courses on screen, so they can be matched to classes.
    var importedCourses: [(id: String, courseID: String, name: String, connection: String)] {
        var seen: [String: (id: String, courseID: String, name: String, connection: String)] = [:]
        for task in library.tasks where !task.courseID.isEmpty {
            let connection = task.extras["canvasConnection"]?.stringValue ?? ""
            let key = connection + ":" + task.courseID
            seen[key] = (key, task.courseID, task.extras["courseName"]?.stringValue ?? task.course, connection)
        }
        return seen.values.sorted { ($0.name, $0.id) < ($1.name, $1.id) }
    }

    // MARK: Completion

    /// Ticking a Canvas assignment marks it done in Canvas too. The local tick
    /// never waits for the network; an unacknowledged change stays on the task
    /// and retries after the next successful import.
    func reportCanvasCompletion(of task: StudentTask) async {
        let generation = canvasGeneration
        let accountID = owner
        if let session = token, accountCanvas?.connected == true {
            guard let intent = task.pendingCanvasDone, !canvasCompletionSending.contains(task.id) else { return }
            canvasCompletionSending.insert(task.id)
            defer { canvasCompletionSending.remove(task.id) }
            await syncNow()
            guard owner == accountID, generation == canvasGeneration, syncStatus == .synced else { return }
            do {
                let reply = try await api.completeAccountCanvas(document: "t_" + task.id, intent: intent, token: session)
                guard owner == accountID, generation == canvasGeneration else { return }
                if reply.module == false { setImportStatus(.failed("Canvas module completion is still pending.")) }
                await syncNow()
            } catch {
                if owner == accountID && generation == canvasGeneration { setImportStatus(.failed("Completion is saved and will retry when Canvas is available.")) }
            }
            return
        }
        guard canvasConnected, let credential = Keychain.read(.canvasToken),
            let plannable = task.canvasPlannable
        else { return }
        mark(taskID: task.id, pending: task.done)
        do {
            try await api.canvasComplete(
                base: canvasBase, token: credential, type: plannable.type, id: plannable.id,
                overrideID: task.overrideID, courseID: task.courseID,
                moduleItem: task.moduleItem, done: task.done)
            guard owner == accountID, generation == canvasGeneration else { return }
            mark(taskID: task.id, pending: nil)
        } catch {
            // Left pending; the next import retries it.
        }
    }

    private func retryPendingCanvasCompletions() async {
        for task in library.tasks where (task.pendingDone != nil || task.pendingCanvasDone != nil) && task.source == .canvas {
            await reportCanvasCompletion(of: task)
        }
    }

    func saveAccountCanvas(_ connection: CanvasConnection) {
        guard accountCanvas != connection else { return }
        accountCanvas = connection
        if connection.connected {
            Keychain.remove(.canvasToken)
            Keychain.remove(.canvasBase)
            setCanvasBase("")
        }
        if let data = try? JSONEncoder().encode(connection) {
            UserDefaults.standard.set(data, forKey: "canvas-connection:" + owner)
        }
    }

    private func mark(taskID: String, pending: Bool?) {
        guard let index = library.tasks.firstIndex(where: { $0.id == taskID }) else { return }
        library.tasks[index].pendingDone = pending
        scheduleSave()
    }
}
