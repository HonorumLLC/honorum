import SwiftUI

/// Account, the app's one menu: who is signed in and their streak, then
/// classes, tasks, notifications, and the rest. Setting up classes lives
/// here, out of the way of the planner.
struct SettingsView: View {
    @Environment(AppState.self) private var state
    @AppStorage("tasksAtDueTime") private var tasksAtDueTime = false
    @State private var confirmingReset = false
    @State private var clearingCompleted = false
    @State private var notificationsOff = false
    @State private var managingSchedules = false

    var body: some View {
        AppList {
            ProfileSection()

            Section {
                NavigationLink(value: ClassesRoute.manage) {
                    LabeledContent {
                        Text("\(state.schedule.sections.count)")
                    } label: {
                        TileLabel("My Classes", "books.vertical", hex: Tiles.blue)
                    }
                }
                .accessibilityIdentifier("manageClasses")
                NavigationLink(value: ClassesRoute.search) {
                    TileLabel("Find Classes", "magnifyingglass", hex: Tiles.blue)
                }
                Button { managingSchedules = true } label: {
                    LabeledContent {
                        Text(state.schedule.name)
                    } label: {
                        TileLabel("Schedules", "square.stack", hex: Tiles.lavender)
                    }
                }
                .foregroundStyle(Color.ink)
                if state.isSignedIn {
                    NavigationLink(value: ClassesRoute.versions) {
                        TileLabel("Earlier Versions", "clock.arrow.circlepath", hex: Tiles.lavender)
                    }
                }
                NavigationLink(value: ClassesRoute.school) {
                    LabeledContent {
                        Text(state.institution?.name ?? "Choose")
                    } label: {
                        TileLabel("School", "building.columns", hex: Tiles.periwinkle)
                    }
                }
                NavigationLink(value: ClassesRoute.term) {
                    LabeledContent {
                        Text(state.term?.name ?? "Choose")
                    } label: {
                        TileLabel("Term", "calendar", hex: Tiles.periwinkle)
                    }
                }
            } header: {
                ListHeader("Classes")
            }

            Section {
                NavigationLink(value: ClassesRoute.sources) {
                    LabeledContent {
                        Text(assignmentSummary)
                    } label: {
                        TileLabel("Assignment Sources", "link", hex: Tiles.mint)
                    }
                }
                if !state.importedCourses.isEmpty {
                    NavigationLink(value: ClassesRoute.canvasCourses) {
                        TileLabel("Canvas Courses", "graduationcap", hex: Tiles.mint)
                    }
                }
                Toggle(isOn: $tasksAtDueTime) {
                    TileLabel("Tasks at Their Due Time", "calendar.day.timeline.left", hex: Tiles.peach)
                }
                .tint(.brand)
            } header: {
                ListHeader("Tasks")
            } footer: {
                Text("Shows each task on the calendar at the hour it is due.")
            }

            Section {
                // One switch covers both kinds, as on the web and the server.
                Toggle(isOn: remindersBinding) {
                    HStack(spacing: 12) {
                        IconTile("bell", hex: Tiles.apricot)
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Reminders").foregroundStyle(Color.ink)
                            Text("Before class and before work is due")
                                .font(.footnote)
                                .foregroundStyle(Color.inkMuted)
                        }
                    }
                }
                .tint(.brand)
                NavigationLink(value: ClassesRoute.alerts) {
                    LabeledContent {
                        Text(alertCount)
                    } label: {
                        TileLabel("Seat Alerts", "chair", hex: Tiles.apricot)
                    }
                }
            } header: {
                ListHeader("Notifications")
            }

            SyncSection()

            Section {
                Link(destination: URL(string: "https://honorum.app/legal.html")!) {
                    TileLabel("Privacy & Terms", "hand.raised", hex: Tiles.neutral)
                }
                LabeledContent {
                    Text(Bundle.main.version)
                } label: {
                    TileLabel("Version", "info.circle", hex: Tiles.neutral)
                }
            } header: {
                ListHeader("About")
            }

            // What cannot be undone comes last, together.
            Section {
                Button("Clear Completed Tasks", role: .destructive) { clearingCompleted = true }
                    .disabled(!state.library.tasks.contains(where: \.done))
                Button(state.isSignedIn ? "Reset Plan" : "Reset Local Plan", role: .destructive) { confirmingReset = true }
                AccountDangerRows()
            }
        }
        .navigationTitle("Account")
        .navigationBarTitleDisplayMode(.large)
        .sheet(isPresented: $managingSchedules) { SchedulesView() }
        .notificationsOffAlert(isPresented: $notificationsOff, purpose: "reminders")
        .confirmationDialog(
            "Clear completed tasks?", isPresented: $clearingCompleted, titleVisibility: .visible
        ) {
            Button("Clear Completed", role: .destructive) {
                for task in state.library.tasks where task.done { state.removeTask(id: task.id) }
            }
        }
        .confirmationDialog(
            "Reset the local plan?", isPresented: $confirmingReset, titleVisibility: .visible
        ) {
            Button("Reset", role: .destructive) {
                let inst = state.library.inst
                let term = state.library.term
                state.library = Library(inst: inst, term: term, onboarded: true)
                state.ensureSchedule()
                state.saveNow()
            }
        } message: {
            Text(
                state.isSignedIn
                    ? "This clears schedules, events, and tasks. These changes also sync to your account."
                    : "This removes the schedules, events, and tasks saved on this device.")
        }
    }

    private var remindersBinding: Binding<Bool> {
        Binding(
            get: { state.library.reminders },
            set: { on in
                Task {
                    // The switch springs back when iOS refuses; say why.
                    if await !state.setReminders(on) { notificationsOff = true }
                }
            })
    }

    private var alertCount: String {
        let count = state.watches.count + state.serverWatches.count
        return count == 0 ? "None" : "\(count)"
    }

    private var assignmentSummary: String {
        if state.canvasConnected && !state.feeds.isEmpty {
            return "Canvas · \(state.feeds.count) feeds"
        }
        if state.canvasConnected { return "Canvas" }
        if !state.feeds.isEmpty { return "\(state.feeds.count) feeds" }
        return "None"
    }
}

/// The schedule's classes, each in its color with when it meets, the way to
/// find more, and the school and term they come from.
struct ManageClassesView: View {
    @Environment(AppState.self) private var state

    var body: some View {
        AppList {
            Section {
                ForEach(state.schedule.sections) { section in
                    HStack(spacing: 12) {
                        Text(section.course.split(separator: " ").first.map(String.init) ?? "")
                            .font(.system(size: 10, weight: .heavy))
                            .foregroundStyle(CoursePalette.mark(state.schedule.color(for: section.course)))
                            .frame(width: 40, height: 40)
                            .background(CoursePalette.fill(state.schedule.color(for: section.course)),
                                        in: .rect(cornerRadius: 12, style: .continuous))
                        VStack(alignment: .leading, spacing: 2) {
                            Text(section.title.isEmpty ? section.course : section.title)
                                .foregroundStyle(Color.ink)
                            Text(detail(section))
                                .font(.footnote)
                                .foregroundStyle(Color.inkMuted)
                                .lineLimit(1)
                        }
                        Spacer(minLength: 4)
                        Button("Remove", systemImage: "minus.circle", role: .destructive) {
                            state.remove(sectionID: section.id)
                        }
                        .labelStyle(.iconOnly)
                        .buttonStyle(.borderless)
                        .foregroundStyle(Color.inkMuted)
                        .accessibilityLabel("Remove \(section.course)")
                    }
                    .padding(.vertical, 2)
                }
                if state.schedule.sections.isEmpty {
                    Text("No classes yet").foregroundStyle(Color.inkMuted)
                }
                NavigationLink(value: ClassesRoute.search) {
                    TileLabel("Find Classes", "plus", hex: Tiles.blue)
                }
                .accessibilityIdentifier("findClasses")
            } header: {
                ListHeader(state.schedule.name)
            } footer: {
                if !state.schedule.sections.isEmpty {
                    let credits = state.schedule.credits
                    Text("\(state.schedule.sections.count) classes · \(credits == credits.rounded() ? String(Int(credits)) : String(format: "%.1f", credits)) credits")
                }
            }
            Section {
                NavigationLink(value: ClassesRoute.school) {
                    LabeledContent {
                        Text(state.institution?.name ?? "Choose")
                    } label: {
                        TileLabel("School", "building.columns", hex: Tiles.periwinkle)
                    }
                }
                NavigationLink(value: ClassesRoute.term) {
                    LabeledContent {
                        Text(state.term?.name ?? "Choose")
                    } label: {
                        TileLabel("Term", "calendar", hex: Tiles.periwinkle)
                    }
                }
            } header: {
                ListHeader("Catalog")
            }
            if !state.skipped(on: state.selectedDate).isEmpty {
                Section {
                    Button("Restore Skipped Meetings") {
                        for item in state.skipped(on: state.selectedDate) {
                            state.restore(itemID: item, on: state.selectedDate)
                        }
                    }
                }
            }
        }
        .navigationTitle("My Classes")
        .navigationBarTitleDisplayMode(.large)
    }

    private func detail(_ section: SavedSection) -> String {
        guard let meeting = section.meetings.first else { return section.course }
        return "\(section.course) · \(Weekdays.describe(meeting.days)) \(Clock.time(meeting.start))"
    }
}

struct SchoolPicker: View {
    @Environment(AppState.self) private var state

    var body: some View {
        AppList {
            ForEach(state.config?.institutions ?? []) { institution in
                Button {
                    state.choose(institution: institution.id)
                } label: {
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(institution.name).foregroundStyle(Color.ink)
                            Text(institution.terms.count == 1 ? "1 term" : "\(institution.terms.count) terms")
                                .font(.caption)
                                .foregroundStyle(Color.inkMuted)
                        }
                        Spacer()
                        if institution.id == state.institution?.id {
                            Image(systemName: "checkmark").foregroundStyle(Color.brand)
                        }
                    }
                }
            }
        }
        .navigationTitle("School")
        .navigationBarTitleDisplayMode(.large)
    }
}

struct TermPicker: View {
    @Environment(AppState.self) private var state

    var body: some View {
        AppList {
            ForEach(state.institution?.terms ?? []) { term in
                Button {
                    state.choose(term: term.id)
                } label: {
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(term.name).foregroundStyle(Color.ink)
                            Text(range(term))
                                .font(.caption)
                                .foregroundStyle(Color.inkMuted)
                        }
                        Spacer()
                        if term.id == state.term?.id {
                            Image(systemName: "checkmark").foregroundStyle(Color.brand)
                        }
                    }
                }
            }
        }
        .navigationTitle("Term")
        .navigationBarTitleDisplayMode(.large)
    }

    private func range(_ term: Term) -> String {
        guard let start = CivilDate(term.start ?? ""), let end = CivilDate(term.end ?? "") else {
            return "\(term.sections ?? 0) sections"
        }
        return "\(start.shortDate) – \(end.shortDate)"
    }
}

#if DEBUG
#Preview("Account") {
    NavigationStack { SettingsView().classesDestinations() }
        .environment(AppState.preview())
        .tint(.brand)
}
#endif

extension Bundle {
    var version: String {
        let short = infoDictionary?["CFBundleShortVersionString"] as? String ?? "1.0"
        let build = infoDictionary?["CFBundleVersion"] as? String ?? "1"
        return "\(short) (\(build))"
    }
}


struct ScheduleHistoryView: View {
    @Environment(AppState.self) private var state
    @State private var versions: [DocumentVersion] = []
    @State private var loading = false
    @State private var error = ""
    @State private var selected: DocumentVersion?
    @State private var accountID = ""
    @State private var scheduleID = ""

    var body: some View {
        AppList {
            if loading { ProgressView("Loading Earlier Versions") }
            if !error.isEmpty {
                Text(error).foregroundStyle(Color.danger)
                Button("Try Again") { Task { await load() } }
            } else if !loading && versions.isEmpty {
                Text("No earlier versions from the last 30 days.")
            }
            ForEach(versions, id: \.rev) { version in
                Button {
                    selected = version
                } label: {
                    VStack(alignment: .leading) {
                        Text(Date(timeIntervalSince1970: Double(version.archived)), format: .dateTime.month().day().hour().minute())
                        if let body = version.body {
                            let plan = Schedule(body)
                            Text("\(plan.name) · \(plan.sections.count) sections · \(plan.events.count) events")
                                .font(.caption).foregroundStyle(Color.inkMuted)
                        }
                    }
                }.disabled(version.deleted == true || version.body == nil)
            }
        }
        .navigationTitle("Earlier Versions")
        .navigationBarTitleDisplayMode(.large)
        .task(id: state.owner + state.schedule.id) { await load() }
        .confirmationDialog("Restore this schedule version?", isPresented: Binding(
            get: { selected != nil }, set: { if !$0 { selected = nil } }
        ), titleVisibility: .visible, presenting: selected) { version in
            Button("Restore Version") {
                if let body = version.body, state.owner == accountID, state.schedule.id == scheduleID,
                    body.text("id") == scheduleID {
                    state.schedule = Schedule(body)
                    state.saveNow()
                    Task { await Reminders.shared.rebuild(from: state) }
                }
                selected = nil
            }
        } message: { _ in
            Text("This replaces the current schedule and syncs to your account. Other schedules and tasks stay as they are.")
        }
    }

    private func load() async {
        selected = nil
        versions = []
        loading = false
        error = ""
        guard let token = state.token else { return }
        let owner = state.owner
        let id = state.schedule.id
        accountID = owner
        scheduleID = id
        loading = true
        do {
            let history = try await state.api.history(document: "s_" + id, token: token)
            guard state.owner == owner, state.schedule.id == id else { return }
            versions = history.versions
        } catch {
            guard state.owner == owner, state.schedule.id == id else { return }
            self.error = error.localizedDescription
        }
        loading = false
    }
}
