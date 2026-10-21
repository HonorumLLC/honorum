import Foundation
import Testing

@testable import Honorum

@Suite("Merging two edits of one document")
struct MergeTests {
    private func json(_ text: String) -> JSON { JSON.decode(Data(text.utf8))! }

    @Test("Independent fields both survive")
    func independentFields() {
        let base = json(#"{"name":"Fall","primary":false}"#)
        let local = json(#"{"name":"Fall 2026","primary":false}"#)
        let remote = json(#"{"name":"Fall","primary":true}"#)
        let result = JSON.merge(base: base, local: local, remote: remote)
        #expect(!result.conflicted)
        #expect(result.body.text("name") == "Fall 2026")
        #expect(result.body.flag("primary") == true)
    }

    @Test("The same field changed on both sides keeps the local edit and reports a conflict")
    func sameField() {
        let base = json(#"{"name":"Fall"}"#)
        let local = json(#"{"name":"Mine"}"#)
        let remote = json(#"{"name":"Theirs"}"#)
        let result = JSON.merge(base: base, local: local, remote: remote)
        #expect(result.conflicted)
        #expect(result.body.text("name") == "Mine")
    }

    @Test("A list of ids takes additions from both sides and honors removals")
    func lists() {
        let base = json(#"{"sections":["a","b"]}"#)
        let local = json(#"{"sections":["a","b","c"]}"#)  // added c
        let remote = json(#"{"sections":["b"]}"#)  // removed a
        let result = JSON.merge(base: base, local: local, remote: remote)
        #expect(!result.conflicted)
        #expect(result.body.list("sections").compactMap(\.stringValue) == ["b", "c"])
    }

    @Test("An unchanged side never overwrites the other")
    func untouched() {
        let base = json(#"{"note":"one"}"#)
        let remote = json(#"{"note":"two"}"#)
        let result = JSON.merge(base: base, local: base, remote: remote)
        #expect(!result.conflicted)
        #expect(result.body.text("note") == "two")
    }

    @Test("Nested objects merge key by key")
    func nested() {
        let base = json(##"{"colors":{"CS":"#cfe6fb"},"removed":{}}"##)
        let local = json(##"{"colors":{"CS":"#fdf4bb","MATH":"#fbd7d7"},"removed":{}}"##)
        let remote = json(##"{"colors":{"CS":"#d3f0da"},"removed":{}}"##)
        let result = JSON.merge(base: base, local: local, remote: remote)
        #expect(result.conflicted)  // both sides recolored CS
        #expect(result.body.map("colors")["CS"]?.stringValue == "#fdf4bb")
        #expect(result.body.map("colors")["MATH"]?.stringValue == "#fbd7d7")
    }

    @Test("A key only one side changed takes that side's value")
    func oneSidedNestedChange() {
        let base = json(##"{"colors":{"CS":"#cfe6fb"}}"##)
        let remote = json(##"{"colors":{"CS":"#d3f0da"}}"##)
        let result = JSON.merge(base: base, local: base, remote: remote)
        #expect(!result.conflicted)
        #expect(result.body.map("colors")["CS"]?.stringValue == "#d3f0da")
    }
}

@Suite("Account documents")
struct DocumentTests {
    /// The shape the web client writes, so both clients read the same account.
    private let scheduleBody = """
        {"id":"s1","name":"Fall","inst":"ou","term":"202610","primary":true,
         "sections":["10001"],
         "pinned":{"10001":{"id":"10001","course":"C S 2334","subject":"C S","title":"Programming",
           "num":"001","type":"LEC","link":"","meetings":[{"days":"MWF","start":540,"end":590,"location":"PSC 100"}],
           "credits":3,"start":"","end":"","instructors":["Dean Hougen"],"mode":"In person",
           "capacity":100,"available":4,"waitlist":0,"url":"https://classnav.ou.edu/"}},
         "removed":{"10001":["2026-09-07"]},
         "blocks":[{"id":"b1","name":"Work","days":"TR","date":"","start":1020,"end":1200,"location":"Union","note":""}],
         "colors":{"C S 2334":"#cfe6fb"},
         "somethingNew":{"kept":true}}
        """

    @Test("A schedule from the account reads back with its sections, blocks and colors")
    func readsWebSchedule() {
        let schedule = Schedule(JSON.decode(Data(scheduleBody.utf8))!)
        #expect(schedule.id == "s1")
        #expect(schedule.name == "Fall")
        #expect(schedule.primary)
        #expect(schedule.sections.count == 1)
        #expect(schedule.sections[0].course == "C S 2334")
        #expect(schedule.sections[0].meetings.first?.location == "PSC 100")
        #expect(schedule.removed["10001"] == ["2026-09-07"])
        #expect(schedule.events.first?.title == "Work")
        #expect(schedule.colors["C S 2334"] == "#cfe6fb")
    }

    @Test("Event classes, color labels and exam sittings round-trip with the web's keys")
    func eventLabelsAndExams() {
        let body = JSON.decode(Data("""
            {"id":"s2","name":"Fall","inst":"ou","term":"202610","primary":true,
             "sections":["10001"],
             "pinned":{"10001":{"id":"10001","course":"C S 2334","num":"001","meetings":[],
               "exams":[{"date":"2026-12-17","start":480,"end":600,"location":"Dale 200"}]}},
             "blocks":[{"id":"b1","name":"Review","days":"","date":"2026-09-11","start":600,"end":660,"location":"","note":"bring notes","course":"C S 2334","color":""},
                       {"id":"b2","name":"Gym","days":"TR","date":"","start":700,"end":760,"location":"","note":"","course":"","color":"#d3f0da"}]}
            """.utf8))!
        let schedule = Schedule(body)
        #expect(schedule.sections[0].exams == [Exam(date: "2026-12-17", start: 480, end: 600, location: "Dale 200")])
        #expect(schedule.events[0].course == "C S 2334")
        #expect(schedule.events[0].notes == "bring notes")
        #expect(schedule.events[1].color == "#d3f0da")
        let written = schedule.body
        #expect(written.map("pinned")["10001"]?.list("exams").first?.text("date") == "2026-12-17")
        #expect(written.list("blocks")[0].text("course") == "C S 2334")
        #expect(written.list("blocks")[1].text("color") == "#d3f0da")
    }

    @Test("A one-time event is written with its weekday, as the web and reminders read it")
    func oneTimeEventCarriesItsWeekday() {
        var once = PersonalEvent()
        once.date = "2026-09-08"
        #expect(once.body.text("days") == "T")
        // Moving the date moves the weekday, even when an older copy kept another.
        once.days = "M"
        once.date = "2026-09-11"
        #expect(once.body.text("days") == "F")
        var weekly = PersonalEvent()
        weekly.days = "TR"
        #expect(weekly.body.text("days") == "TR")
        // An event saved without its weekday differs from what is written now,
        // so the next exchange uploads the corrected copy.
        let stored = JSON.decode(Data(#"{"id":"b1","name":"Review","days":"","date":"2026-09-11","start":600,"end":660,"location":"","note":"","course":"","color":""}"#.utf8))!
        #expect(PersonalEvent(stored).body.canonical != stored.canonical)
        let rewritten = PersonalEvent(stored).body
        #expect(PersonalEvent(rewritten).body.canonical == rewritten.canonical)
    }

    @Test("Writing it back keeps the ordered ids, the pinned copies, and unknown fields")
    func writesWebSchedule() {
        let original = JSON.decode(Data(scheduleBody.utf8))!
        let body = Schedule(original).body
        #expect(body.list("sections").compactMap(\.stringValue) == ["10001"])
        #expect(body.map("pinned")["10001"]?.text("course") == "C S 2334")
        #expect(body["somethingNew"] == original["somethingNew"])
        // A round trip is stable, so an untouched document never uploads again.
        #expect(Schedule(body).body.canonical == body.canonical)
    }

    @Test("A task keeps its upstream identity and an unknown field")
    func task() {
        let body = JSON.decode(
            Data(
                #"{"id":"t1","title":"Essay","course":"ENGL 1213","due":"2026-09-15T17:00:00Z","note":"draft","done":false,"source":"canvas","uid":"canvas:assignment:9","overrideId":"77","future":1}"#
                    .utf8))!
        let task = StudentTask(body)
        #expect(task.uid == "canvas:assignment:9")
        #expect(task.canvasPlannable?.type == "assignment")
        #expect(task.canvasPlannable?.id == "9")
        #expect(task.dueDate != nil)
        #expect(task.body["future"] == .number(1))
        #expect(StudentTask(task.body).body.canonical == task.body.canonical)
    }

    @Test("Preferences carry the school, term, open schedule and watchlist")
    func preferences() {
        var library = Library()
        library.inst = "ou"
        library.term = "202610"
        library.current = "s1"
        library.onboarded = true
        library.watching = [
            SeatWatch(inst: "ou", term: "202610", section: "10001", label: "C S 2334-001", title: "Programming")
        ]
        var restored = Library()
        restored.apply(prefs: library.prefsBody, restoringNavigation: true)
        #expect(restored.inst == "ou")
        #expect(restored.term == "202610")
        #expect(restored.current == "s1")
        #expect(restored.onboarded)
        #expect(restored.watching.count == 1)

        // A later poll must not move the student off the term they are on.
        var elsewhere = Library()
        elsewhere.inst = "tamu"
        elsewhere.apply(prefs: library.prefsBody, restoringNavigation: false)
        #expect(elsewhere.inst == "tamu")
    }
}

@Suite("The document exchange")
struct ExchangeTests {
    private func library() -> Library {
        var library = Library()
        library.inst = "ou"
        library.term = "202610"
        var plan = Schedule(name: "Fall", inst: "ou", term: "202610", primary: true)
        plan.id = "s1"
        library.schedules = [plan]
        library.current = "s1"
        var task = StudentTask()
        task.id = "t1"
        task.title = "Essay"
        library.tasks = [task]
        return library
    }

    @Test("A fresh library uploads a schedule, a task, and its preferences")
    func firstUpload() {
        let documents = Sync.documents(of: library())
        #expect(Set(documents.keys) == ["s_s1", "t_t1", "prefs"])
        let pending = Sync.pending(documents, known: [:], updated: 1)
        #expect(pending.count == 3)
        #expect(pending.allSatisfy { $0.base == 0 })
    }

    @Test("Nothing uploads twice while it is unchanged")
    func steadyState() {
        let documents = Sync.documents(of: library())
        var known: [String: SyncState.Known] = [:]
        for (id, document) in documents {
            known[id] = SyncState.Known(
                rev: 4, type: document.type, deleted: false, body: document.body.canonical)
        }
        #expect(Sync.pending(documents, known: known, updated: 2).isEmpty)
    }

    @Test("Removing a schedule uploads a deletion for it")
    func deletion() {
        var known: [String: SyncState.Known] = [
            "s_gone": SyncState.Known(rev: 7, type: "schedule", deleted: false, body: "{}")
        ]
        for (id, document) in Sync.documents(of: library()) {
            known[id] = SyncState.Known(
                rev: 4, type: document.type, deleted: false, body: document.body.canonical)
        }
        let pending = Sync.pending(Sync.documents(of: library()), known: known, updated: 3)
        #expect(pending.count == 1)
        #expect(pending[0].id == "s_gone")
        #expect(pending[0].deleted == true)
        #expect(pending[0].base == 7)
    }

    @Test("A downloaded schedule lands in the library, and a deletion removes it")
    func applying() {
        var library = library()
        var incoming = Schedule(name: "Spring", inst: "ou", term: "202620", primary: false)
        incoming.id = "s2"
        Sync.apply(
            SyncDoc(id: "s_s2", type: "schedule", rev: 9, body: incoming.body), to: &library,
            restoringNavigation: false)
        #expect(library.schedules.count == 2)

        Sync.apply(
            SyncDoc(id: "s_s2", type: "schedule", rev: 10, deleted: true), to: &library,
            restoringNavigation: false)
        #expect(library.schedules.map(\.id) == ["s1"])
    }

    @Test("A stale upload merges with the version the server already had")
    func conflictResolution() {
        var mine = Schedule(name: "Fall", inst: "ou", term: "202610", primary: true)
        mine.id = "s1"
        let base = mine
        mine.name = "My Fall"

        var theirs = base
        theirs.colors = ["C S 2334": "#cfe6fb"]

        let resolved = Sync.resolve(
            local: SyncDoc(id: "s_s1", type: "schedule", body: mine.body),
            remote: SyncDoc(id: "s_s1", type: "schedule", rev: 12, body: theirs.body),
            base: SyncState.Known(rev: 11, type: "schedule", deleted: false, body: base.body.canonical))

        #expect(resolved.conflict == nil)
        let merged = Schedule(resolved.merged)
        #expect(merged.name == "My Fall")
        #expect(merged.colors["C S 2334"] == "#cfe6fb")
    }

    @Test("A batch stays inside the server's document and byte limits")
    func batching() {
        let documents = (0..<500).map { index in
            SyncDoc(
                id: "t_\(index)", type: "task", base: 0, updated: 1,
                body: .object(["title": .of("Task \(index)")]))
        }
        #expect(Sync.batch(documents).count == Sync.maxDocumentsPerRequest)
    }
}

struct SharedAccountContractTests {
    @Test("Web completion intents survive native editing and local persistence")
    func canvasIntent() throws {
        let raw = #"{"id":"shared","source":"canvas","uid":"canvas:assignment:42","title":"Essay","done":false,"pendingCanvasDone":"intent-from-web","canvasConnection":"connection","note":"keep"}"#
        var task = StudentTask(JSON.decode(Data(raw.utf8))!)
        #expect(task.pendingCanvasDone == "intent-from-web")
        task.notes = "Edited on iOS"
        let restored = try JSONDecoder().decode(StudentTask.self, from: JSONEncoder().encode(task))
        #expect(restored.body.text("pendingCanvasDone") == "intent-from-web")
        #expect(restored.body.text("canvasConnection") == "connection")
        #expect(restored.body.text("note") == "Edited on iOS")
    }

    @Test("Account API status decodes the shared streak and secret-free connection")
    func statusContract() throws {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let streak = try decoder.decode(AccountStreak.self, from: Data(#"{"current":3,"longest":8,"today":"2026-09-11","last_day":"2026-09-11","timezone":"America/Chicago"}"#.utf8))
        #expect(streak.current == 3 && streak.lastDay == "2026-09-11")
        let connection = try decoder.decode(CanvasConnection.self, from: Data(#"{"id":"connection","base":"https://canvas.example.edu","connected":true,"available":true,"synced":1789131600,"error":""}"#.utf8))
        #expect(connection.connected && connection.base == "https://canvas.example.edu")
    }
}

@MainActor
struct AccountImportMergeTests {
    @Test("An import received during an edit preserves the edit and adopts Canvas fields")
    func editDuringImport() {
        let state = AppState()
        var base = StudentTask()
        base.id = "shared"
        base.title = "Essay"
        base.source = .canvas
        base.uid = "canvas:assignment:42"
        let doc = SyncDoc(id: "t_shared", type: "task", rev: 1, body: base.body)
        state.syncState.known[doc.id] = Sync.remember(doc, rev: 1)
        var local = base
        local.notes = "Typed while Canvas was loading"
        local.pendingCanvasDone = "new-intent"
        local.done = true
        state.library.tasks = [local]
        var remote = base
        remote.due = "2026-09-12T18:00:00Z"
        let incoming = SyncDoc(id: doc.id, type: "task", rev: 2, body: remote.body)
        state.apply(SyncReply(seq: 2, docs: [incoming]), sent: [])
        #expect(state.library.tasks[0].notes == local.notes)
        #expect(state.library.tasks[0].pendingCanvasDone == "new-intent")
        #expect(state.library.tasks[0].done)
        #expect(state.library.tasks[0].due == remote.due)
    }

    @Test("A conflict repeated in the download cannot overwrite its merged result")
    func repeatedConflict() {
        let state = AppState()
        var base = StudentTask()
        base.id = "shared"
        let doc = SyncDoc(id: "t_shared", type: "task", rev: 1, body: base.body)
        state.syncState.known[doc.id] = Sync.remember(doc, rev: 1)
        var sent = base
        sent.notes = "Sent edit"
        var latest = sent
        latest.notes = "Newer unsent edit"
        state.library.tasks = [latest]
        var remote = base
        remote.course = "BIO 101"
        let incoming = SyncDoc(id: doc.id, type: "task", rev: 2, body: remote.body)
        state.apply(SyncReply(seq: 2, docs: [incoming], conflicts: [incoming]), sent: [SyncDoc(id: doc.id, type: "task", base: 1, body: sent.body)])
        #expect(state.library.tasks[0].notes == latest.notes)
        #expect(state.library.tasks[0].course == remote.course)
    }
}

@MainActor
@Suite("The first download after signing in")
struct FirstDownloadTests {
    /// A device part-way through setup: the first school listed, adopted as a
    /// default, with the empty schedule made for it and the tour unfinished.
    private func guestDevice() -> AppState {
        let state = AppState()
        state.library.inst = "ou"
        state.library.term = "202610"
        var plan = Schedule(name: "Schedule 1", inst: "ou", term: "202610", primary: true)
        plan.id = "guest"
        state.library.schedules = [plan]
        state.library.current = "guest"
        return state
    }

    /// The account's preferences and the schedule they open, at another school.
    private func accountDocs(onboarded: Bool) -> [SyncDoc] {
        var account = Library()
        account.inst = "tamu"
        account.term = "202631"
        account.current = "fall"
        account.onboarded = onboarded
        var fall = Schedule(name: "Fall", inst: "tamu", term: "202631", primary: true)
        fall.id = "fall"
        return [
            SyncDoc(id: "prefs", type: "prefs", rev: 4, body: account.prefsBody),
            SyncDoc(id: "s_fall", type: "schedule", rev: 3, body: fall.body),
        ]
    }

    @Test("The account's school and finished tour replace the defaults this device adopted")
    func accountPreferencesWin() {
        let state = guestDevice()
        state.apply(SyncReply(seq: 4, docs: accountDocs(onboarded: true)), sent: [])
        #expect(state.library.onboarded)
        #expect(state.library.inst == "tamu")
        #expect(state.library.term == "202631")
        #expect(state.library.current == "fall")
        #expect(state.syncState.conflicts.isEmpty)
        #expect(!state.syncState.needsFirstPull)
    }

    @Test("An account that has planned skips setup, and the empty schedule made here is dropped")
    func plansFinishSetup() {
        let state = guestDevice()
        var fall = Schedule(name: "Fall", inst: "ou", term: "202610", primary: true)
        fall.id = "fall"
        var essay = StudentTask()
        essay.id = "essay"
        essay.title = "Essay"
        var account = Library()
        account.inst = "ou"
        account.term = "202610"
        account.current = "fall"
        let docs = [
            SyncDoc(id: "prefs", type: "prefs", rev: 1, body: account.prefsBody),
            SyncDoc(id: "s_fall", type: "schedule", rev: 2, body: fall.body),
            SyncDoc(id: "t_essay", type: "task", rev: 3, body: essay.body),
        ]
        let waiting = state.apply(SyncReply(seq: 3, docs: docs), sent: [])
        #expect(state.library.onboarded)
        #expect(state.library.schedules.map(\.id) == ["fall"])
        #expect(state.library.current == "fall")
        // The finished tour is this device's news, and all it has to upload.
        #expect(waiting)
        let pending = Sync.pending(Sync.documents(of: state.library), known: state.syncState.known, updated: 1)
        #expect(pending.map(\.id) == ["prefs"])
    }

    @Test("A schedule made here survives when the account has none for its term, or when it has work in it")
    func ownSchedulesSurvive() {
        let state = guestDevice()
        var elsewhere = Schedule(name: "Spring", inst: "ou", term: "202620", primary: true)
        elsewhere.id = "spring"
        state.apply(
            SyncReply(seq: 2, docs: [SyncDoc(id: "s_spring", type: "schedule", rev: 2, body: elsewhere.body)]),
            sent: [])
        #expect(Set(state.library.schedules.map(\.id)) == ["guest", "spring"])

        let busy = guestDevice()
        busy.library.schedules[0].events = [PersonalEvent()]
        var fall = Schedule(name: "Fall", inst: "ou", term: "202610", primary: true)
        fall.id = "fall"
        busy.apply(
            SyncReply(seq: 2, docs: [SyncDoc(id: "s_fall", type: "schedule", rev: 2, body: fall.body)]),
            sent: [])
        #expect(Set(busy.library.schedules.map(\.id)) == ["guest", "fall"])
    }

    @Test("A new account leaves setup where it was")
    func newAccountKeepsSetup() {
        let state = guestDevice()
        state.apply(SyncReply(seq: 0), sent: [])
        #expect(!state.library.onboarded)
        #expect(state.library.inst == "ou")
        #expect(state.library.schedules.map(\.id) == ["guest"])
    }

    @Test("Later downloads keep a local preference the account disagrees with")
    func laterDownloadsKeepLocalEdits() {
        let state = guestDevice()
        state.syncState.needsFirstPull = false
        state.apply(SyncReply(seq: 4, docs: accountDocs(onboarded: false)), sent: [])
        #expect(state.library.inst == "ou")
        #expect(state.library.current == "guest")
        #expect(state.syncState.conflicts.count == 1)
    }
}

@Suite("Recovering saved copies")
struct RecoveryTests {
    @Test("Recovering a Canvas task creates an independent task without replaying completion")
    func taskCopy() {
        var original = StudentTask()
        original.title = "Essay"
        original.source = .canvas
        original.notes = "Recovered notes"
        original.course = "ENGL 101"
        original.pendingCanvasDone = "pending-intent"
        original.extras["canvasConnection"] = .of("connection")
        original.extras["futureField"] = .of("keep")
        var library = Library()
        library.tasks = [original]
        let copy = ConflictCopy(id: "copy", document: "t_" + original.id, kind: "task", rev: 1, at: Date(), body: original.body)
        #expect(Sync.recover(copy, in: &library))
        let restored = library.tasks[1]
        #expect(library.tasks[0] == original)
        #expect(restored.id != original.id)
        #expect(restored.notes == original.notes && restored.course == original.course)
        #expect(restored.source == .manual && restored.pendingCanvasDone == nil)
        #expect(restored.extras["canvasConnection"] == nil)
        #expect(restored.extras["futureField"] == .of("keep"))
    }

    @Test("A recovered schedule does not replace the primary schedule or current navigation")
    func scheduleCopy() {
        let original = Schedule(name: "Fall", inst: "test", term: "fall", primary: true)
        var library = Library()
        library.schedule = original
        let current = library.current
        let copy = ConflictCopy(id: "copy", document: "s_" + original.id, kind: "schedule", rev: 1, at: Date(), body: original.body)
        #expect(Sync.recover(copy, in: &library))
        #expect(library.current == current)
        #expect(library.schedules.last?.id != original.id)
        #expect(library.schedules.last?.primary == false)
        #expect(library.schedules.last?.name == "Fall (recovered)")
    }

    @Test("Settings and deleted copies cannot replace local preferences")
    func unsupportedCopies() {
        var library = Library(inst: "test")
        let before = library
        let copy = ConflictCopy(id: "copy", document: "prefs", kind: "prefs", rev: 1, at: Date(), body: .object(["inst": .of("other")]))
        #expect(!Sync.recover(copy, in: &library))
        let deleted = ConflictCopy(id: "deleted", document: "t_old", kind: "task", rev: 2, at: Date(), body: .object([:]))
        #expect(!Sync.recover(deleted, in: &library))
        #expect(library.prefsBody == before.prefsBody)
        #expect(library.tasks.isEmpty && library.schedules.isEmpty)
    }
}
