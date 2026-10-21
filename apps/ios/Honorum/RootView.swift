import SwiftUI

/// Home, and the screens it pushes on one stack: the calendar, every task,
/// Account, and the class screens Account holds. The New sheet and the task
/// editor are sheets over all of them.
struct RootView: View {
    @Environment(AppState.self) private var state
    @State private var routes: [ClassesRoute] = RootView.launchRoutes
    @State private var event: PersonalEvent?
    @State private var task: StudentTask?
    @State private var composing: Composing?

    /// A New sheet, opening on a task or an event.
    struct Composing: Identifiable {
        let id = UUID()
        let kind: NewKind
        let event: PersonalEvent
        var task = StudentTask()
    }
    #if DEBUG
    @State private var debugSheet: DebugSheet?
    #endif

    /// `-startTab schedule|tasks|account` opens on that screen, over Home,
    /// for UI tests and screenshots.
    static var launchRoutes: [ClassesRoute] {
        #if DEBUG
        if let routes = DebugScreen.routes { return routes }
        #endif
        return switch UserDefaults.standard.string(forKey: "startTab") {
        case "schedule": [.schedule]
        case "tasks": [.tasks]
        case "account", "settings": [.account]
        default: []
        }
    }

    var body: some View {
        if !state.loaded {
            // The launch screen's surface, until the saved plan says which
            // screen this is; a returning student never sees the tour flash.
            Color.surface.ignoresSafeArea()
        } else if state.library.onboarded {
            NavigationStack(path: $routes) {
                HomeView(open: open, edit: { task = $0 }, compose: { compose(.task) })
                    .classesDestinations(routes: $routes, compose: compose)
            }
            .task(id: state.catalogKey) { await state.loadCatalog() }
            .sheet(item: $event) { EventEditor(event: $0) }
            .sheet(item: $task) { TaskEditor(task: $0) }
            .sheet(item: $composing) { NewItemSheet(kind: $0.kind, event: $0.event, task: $0.task) }
            .onAppear {
                // Setup ended on Search Classes: the search takes over.
                if state.editClassesOnOpen {
                    state.editClassesOnOpen = false
                    routes = [.manage, .search]
                }
            }
            .userNoticeAlert()
            .onChange(of: state.pendingRoute, initial: true) { _, route in follow(route) }
            #if DEBUG
            .sheet(item: $debugSheet) { DebugSheetContent(sheet: $0) }
            .task { openDebugSheet() }
            #endif
        } else {
            WelcomeView()
                .userNoticeAlert()
        }
    }

    /// Push a screen once; a second tap while it is on top is a no-op.
    private func open(_ route: ClassesRoute) {
        guard routes.last != route else { return }
        routes.append(route)
    }

    private func follow(_ route: PendingRoute?) {
        guard let route else { return }
        switch route {
        case .day(let date):
            state.selectedDate = date
            routes = [.schedule]
        case .task(let id):
            routes = []
            if let found = state.library.tasks.first(where: { $0.id == id }) { task = found }
        case .alerts:
            routes = [.account]
        }
        state.pendingRoute = nil
    }

    #if DEBUG
    private func openDebugSheet() {
        switch DebugScreen.name {
        case "newTask": compose(.task)
        case "editTask": task = state.library.tasks.first { !$0.done && $0.source == .canvas }
        case "newEvent": compose(.event)
        case "newTaskSample", "newEventSample":
            // A filled-in sheet, with the keyboard down.
            var sample = StudentTask()
            sample.title = DebugScreen.name == "newTaskSample" ? "Lab Report 4" : "Office Hours"
            sample.course = "CHEM 1315"
            sample.due = ISO8601.string(from: state.today.adding(days: 1).instant(minute: 17 * 60, in: state.timeZone))
            var event = PersonalEvent()
            event.title = "Office Hours"
            event.location = "Physical Sciences 210"
            event.course = "CHEM 1315"
            event.date = state.today.adding(days: 1).iso
            event.start = 14 * 60
            event.end = 15 * 60
            composing = Composing(kind: DebugScreen.name == "newTaskSample" ? .task : .event, event: event, task: sample)
        case "editEvent": event = state.schedule.events.first
        case let name where DebugScreen.sheets.contains(name): debugSheet = DebugSheet(id: name)
        default: break
        }
    }
    #endif

    /// The New sheet. An event starts on the calendar's day at the next half
    /// hour, within the waking day.
    private func compose(_ kind: NewKind) {
        var draft = PersonalEvent()
        draft.date = state.selectedDate.iso
        draft.start = min(23 * 60, max(8 * 60, (state.clock.minute / 30 + 1) * 30))
        draft.end = draft.start + 60
        composing = Composing(kind: kind, event: draft)
    }
}
