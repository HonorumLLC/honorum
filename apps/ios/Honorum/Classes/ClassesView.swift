import SwiftUI

/// The screens Home pushes by value: the schedule, every task, Account, and
/// the class screens (Manage Classes, the search, and a course). The app's
/// one stack registers these once, at its root, with `classesDestinations`,
/// so a push from the search and a pop from a course are ordinary stack
/// operations that never touch the screen that opened the search. The whole
/// chain is value-based because a value link inside a screen pushed by a
/// view-builder link never pushes.
enum ClassesRoute: Hashable {
    case schedule
    case tasks
    case account
    case manage
    case search
    case course(Course)
    // Account's own pages.
    case school
    case term
    case sources
    case canvasCourses
    case alerts
    case devices
    case copies
    case versions
}

extension View {
    /// Register the pushed screens with a navigation stack. Apply to the
    /// stack's root, never inside the search itself: a destination declared
    /// on a pushed screen is torn down with it. The schedule and the task
    /// list carry their own add buttons, which the root presents.
    func classesDestinations(
        routes: Binding<[ClassesRoute]> = .constant([]),
        compose: @escaping (NewKind) -> Void = { _ in }
    ) -> some View {
        navigationDestination(for: ClassesRoute.self) { route in
            switch route {
            case .schedule:
                ScheduleView(routes: routes)
                    .actionBar(title: "New Event") { compose(.event) }
            case .tasks:
                TasksView()
                    .actionBar(title: "New Task") { compose(.task) }
            case .account: SettingsView()
            case .manage: ManageClassesView()
            case .search: ClassesView()
            case .course(let course): CourseDetailView(course: course)
            case .school: SchoolPicker()
            case .term: TermPicker()
            case .sources: ConnectView()
            case .canvasCourses: CourseMatchView()
            case .alerts: AlertsView()
            case .devices: DevicesView()
            case .copies: ConflictsView()
            case .versions: ScheduleHistoryView()
            }
        }
    }
}

/// Class search. Every keystroke queries the term's catalog on a background
/// actor and shows a page of forty courses; scrolling asks for the next page.
///
/// Editing classes is a screen the planner pushes, with ordinary back
/// navigation, rather than a tab of its own.
struct ClassesView: View {
    @Environment(AppState.self) private var state

    @State private var query = ""
    @State private var filters = CatalogFilters()
    /// Nothing until the first search answers, so a loaded catalog does not
    /// flash "No Results" on the way in.
    @State private var results: CatalogSearch?
    @State private var limit = pageSize
    @State private var searching: Task<Void, Never>?

    private static let pageSize = 40

    var body: some View {
        list
            .overlay { statusOverlay }
            .navigationTitle("Find Classes")
            .navigationBarTitleDisplayMode(.large)
            .toolbar { toolbar }
            .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always),
                        prompt: "Search \(state.term?.name ?? "classes")")
            .autocorrectionDisabled()
            .textInputAutocapitalization(.never)
        // The planner usually has the catalog loaded already; otherwise load
        // it here, and again whenever Settings changes the term.
        .task(id: state.catalogKey) {
            if state.catalogStatus != .ready { await state.loadCatalog() }
            run()
        }
        .onChange(of: query) { _, _ in limit = Self.pageSize; run(debounce: true) }
        .onChange(of: filters) { _, _ in limit = Self.pageSize; run() }
    }

    @ViewBuilder private var statusOverlay: some View {
        switch state.catalogStatus {
        case .loading, .idle:
            loading
        case .failed(let message):
            ContentUnavailableView {
                Label("Classes Unavailable", systemImage: "wifi.exclamationmark")
            } description: {
                Text(message)
            } actions: {
                Button("Try Again") { Task { await state.loadCatalog(); run() } }
                    .buttonStyle(.borderedProminent)
            }
        case .ready:
            if let results, results.courses.isEmpty {
                if !filters.isDefault {
                    // The filters, not the words, are what left nothing.
                    ContentUnavailableView {
                        Label("No Classes Match", systemImage: "line.3.horizontal.decrease.circle")
                    } description: {
                        Text(query.isEmpty
                            ? "Nothing in the term fits these filters."
                            : "Nothing matching “\(query)” fits these filters.")
                    } actions: {
                        Button("Clear Filters") { filters = CatalogFilters() }
                            .buttonStyle(.borderedProminent)
                    }
                } else if query.isEmpty {
                    ContentUnavailableView("No Classes", systemImage: "books.vertical",
                                           description: Text("The catalog for this term is empty."))
                } else {
                    ContentUnavailableView.search(text: query)
                }
            } else if results == nil {
                // The catalog is ready; its first answer is on the way.
                ProgressView()
            }
        }
    }

    private var loading: some View {
        VStack(spacing: 10) {
            ProgressView()
            Text(state.term.map { "Loading \($0.name)…" } ?? "Loading classes…")
                .font(.footnote)
                .foregroundStyle(Color.inkMuted)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var list: some View {
        AppList {
            if state.catalogStatus == .ready, let results, !results.courses.isEmpty {
                Section {
                    ForEach(results.courses) { course in
                        NavigationLink(value: ClassesRoute.course(course)) {
                            CourseRow(course: course, seats: results.seats)
                        }
                    }
                    if results.courses.count < results.total {
                        HStack {
                            Spacer()
                            ProgressView()
                            Spacer()
                        }
                        .listRowSeparator(.hidden)
                        .onAppear { limit += Self.pageSize; run() }
                    }
                } header: {
                    ListHeader(summary(results))
                }
            }
        }
        .accessibilityIdentifier("classes-list")
    }

    private func summary(_ results: CatalogSearch) -> String {
        let shown = results.courses.count
        if results.total <= shown {
            return "\(results.total) \(results.total == 1 ? "course" : "courses")"
        }
        return "\(shown) of \(results.total) courses"
    }

    @ToolbarContentBuilder private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .topBarTrailing) {
            FilterMenu(filters: $filters, modes: state.catalogModes)
        }
    }

    /// Coalesces typing into one search. The catalog actor does the work, so a
    /// slow query never touches the frame the keyboard is animating.
    private func run(debounce: Bool = false) {
        searching?.cancel()
        let query = query, filters = filters, limit = limit
        searching = Task {
            if debounce { try? await Task.sleep(for: .milliseconds(120)) }
            guard !Task.isCancelled else { return }
            let found = await state.search(query, filters: filters, limit: limit)
            guard !Task.isCancelled else { return }
            results = found
        }
    }
}

/// One course in the results: its code, title, and how its sections stand,
/// marked in its color once it is in the schedule.
struct CourseRow: View {
    @Environment(AppState.self) private var state
    let course: Course
    let seats: [String: Seats]

    var body: some View {
        let saved = state.schedule.sections.contains { $0.course == course.code }
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(course.code)
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(Color.brandInk)
                Text(course.title)
                    .font(.body.weight(.medium))
                    .foregroundStyle(Color.ink)
                    .lineLimit(2)
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(Color.inkMuted)
            }
            Spacer(minLength: 4)
            if saved {
                Image(systemName: "checkmark")
                    .font(.caption.weight(.heavy))
                    .foregroundStyle(CoursePalette.tone(state.schedule.color(for: course.code)))
                    .frame(width: 26, height: 26)
                    .background(CoursePalette.fill(state.schedule.color(for: course.code)), in: .circle)
                    .accessibilityLabel("In your schedule")
            }
        }
        .padding(.vertical, 4)
    }

    private var detail: String {
        let count = course.sections.count
        let open = course.sections.filter { seats[$0.id]?.isOpen ?? false }.count
        var parts = ["\(count) \(count == 1 ? "section" : "sections")"]
        if open > 0 { parts.append("\(open) open") }
        if let credits = course.sections.first?.credits, credits > 0 {
            parts.append("\(credits.formatted()) cr")
        }
        return parts.joined(separator: " · ")
    }
}

/// The filter set, as the menu Apple's apps put behind a toolbar control.
struct FilterMenu: View {
    @Binding var filters: CatalogFilters
    let modes: [String]

    var body: some View {
        Menu {
            Toggle("Open seats only", isOn: $filters.openOnly)
            Picker("Time", selection: $filters.time) {
                ForEach(CatalogFilters.TimeWindow.allCases, id: \.self) { window in
                    Text(window.label).tag(window)
                }
            }
            Picker("Level", selection: $filters.level) {
                Text("Any level").tag("")
                ForEach(1...4, id: \.self) { level in
                    Text("\(level)000 level").tag(String(level))
                }
                Text("Graduate").tag("grad")
            }
            if !modes.isEmpty {
                Picker("Mode", selection: $filters.mode) {
                    Text("Any mode").tag("")
                    ForEach(modes, id: \.self) { mode in Text(mode).tag(mode) }
                }
            }
            Menu("Days") {
                ForEach(Array(Weekdays.letters.enumerated()), id: \.offset) { index, letter in
                    Toggle(Weekdays.names[index], isOn: day(letter))
                }
            }
            if !filters.isDefault {
                Divider()
                Button("Clear Filters", systemImage: "arrow.uturn.backward") {
                    filters = CatalogFilters()
                }
            }
        } label: {
            Label(
                "Filters",
                systemImage: filters.isDefault
                    ? "line.3.horizontal.decrease.circle"
                    : "line.3.horizontal.decrease.circle.fill")
                .foregroundStyle(Color.toolbarIcon)
        }
        .tint(Color.toolbarIcon)
        .accessibilityValue(filters.activeCount > 0 ? "\(filters.activeCount) active" : "")
    }

    private func day(_ letter: Character) -> Binding<Bool> {
        Binding(
            get: { filters.days.contains(letter) },
            set: { on in
                if on {
                    filters.days = Weekdays.letters.filter {
                        filters.days.contains($0) || $0 == letter
                    }
                } else {
                    filters.days.removeAll { $0 == letter }
                }
            })
    }
}
