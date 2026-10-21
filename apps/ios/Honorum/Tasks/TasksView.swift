import SwiftUI

/// Every task on one page, in Home's cards: grouped by when it is due, with
/// a row of class chips to narrow the list and finished work folded at the
/// end. Searching is Home's.
struct TasksView: View {
    @Environment(AppState.self) private var state
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    @State private var refreshError = ""
    @AppStorage("activeTaskSort") private var sort = TaskSort.due
    @AppStorage("activeTaskSortReversed") private var reversed = false
    @State private var courseFilter = ""
    @State private var editing: StudentTask?
    @State private var deleting: StudentTask?
    @State private var showsDone = false
    /// Tasks just ticked, held in place long enough to see the check land.
    @State private var lingering: Set<String> = []
    @State private var lastCompleted: String?

    var body: some View {
        let groups = groups()
        let done = finished()
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                chips
                    .padding(.top, 4)
                if groups.isEmpty && done.isEmpty {
                    empty.padding(.top, 60)
                }
                ForEach(groups, id: \.id) { group in
                    VStack(alignment: .leading, spacing: 10) {
                        if !group.title.isEmpty {
                            Text(group.title)
                                .font(.display(17, relativeTo: .headline))
                                .foregroundStyle(group.id == "Overdue" ? Color.danger : Color.ink)
                                .padding(.horizontal, 4)
                                .accessibilityAddTraits(.isHeader)
                        }
                        card(group.items, detail: group.detail)
                    }
                    .padding(.top, 24)
                }
                if !done.isEmpty {
                    VStack(alignment: .leading, spacing: 10) {
                        VStack(spacing: 0) {
                            CompletedFold(count: done.count, open: $showsDone)
                            if showsDone {
                                ForEach(done) { item in
                                    RowRule(inset: 52)
                                    row(item, detail: item.task.completedAt.map { "Done \(CivilDate($0, in: state.timeZone).monthDay)" })
                                }
                            }
                        }
                        .card(padding: 0)
                    }
                    .padding(.top, 24)
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 24)
            .frame(maxWidth: 640)
            .frame(maxWidth: .infinity)
        }
        .scrollIndicators(.hidden)
        .background(Color.surface.ignoresSafeArea())
        .accessibilityIdentifier("tasks-list")
        .accessibilityValue(refreshStatus)
        .navigationTitle("Tasks")
        .navigationBarTitleDisplayMode(.large)
        .toolbar { toolbar }
        .sensoryFeedback(.success, trigger: lastCompleted)
        .sheet(item: $editing) { TaskEditor(task: $0) }
        .confirmationDialog(
            "Delete this task?",
            isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
            titleVisibility: .visible, presenting: deleting
        ) { task in
            Button("Delete Task", role: .destructive) { state.removeTask(id: task.id) }
        } message: { task in
            Text(task.title)
        }
        .task { await state.refreshAssignments() }
        .refreshable {
            await state.refreshAssignments(force: true)
            if case .failed(let message) = state.importStatus { refreshError = message }
        }
        .alert("Could Not Refresh", isPresented: Binding(get: { !refreshError.isEmpty }, set: { if !$0 { refreshError = "" } })) {
            Button("OK") { refreshError = "" }
        } message: { Text(refreshError) }
    }

    // MARK: Pieces

    /// All, each class in its color, and tasks with no class.
    private var chips: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 8) {
                Chip(title: "All", selected: courseFilter.isEmpty) { courseFilter = "" }
                ForEach(state.taskCourses.candidates, id: \.self) { code in
                    Chip(title: name(code), hex: state.schedule.color(for: code), selected: courseFilter == code) {
                        courseFilter = courseFilter == code ? "" : code
                    }
                    .accessibilityLabel(code)
                }
                Chip(title: "No class", selected: courseFilter == "__none") {
                    courseFilter = courseFilter == "__none" ? "" : "__none"
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 1)
        }
        .scrollIndicators(.hidden)
        .padding(.horizontal, -16)
        .accessibilityIdentifier("task-filter")
    }

    private func card(_ items: [TaskItem], detail: @escaping (TaskItem) -> String?) -> some View {
        VStack(spacing: 0) {
            ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                if index > 0 { RowRule(inset: 52) }
                row(item, detail: detail(item))
            }
        }
        .padding(.vertical, 2)
        .card(padding: 0)
    }

    private func row(_ item: TaskItem, detail: String?) -> some View {
        TaskLine(item: item, detail: detail, detailIsUrgent: item.overdue,
                 onToggle: { complete(item.task) }, onOpen: { editing = item.task })
            .contextMenu {
                if item.task.source == .canvas, item.task.sourceURL != nil {
                    TaskSourceLink(task: item.task) {
                        Label("Open in Canvas", systemImage: "arrow.up.right.square")
                    }
                }
                Button("Delete", systemImage: "trash", role: .destructive) { deleting = item.task }
            }
    }

    private var refreshStatus: String {
        switch state.importStatus {
        case .importing: "Refreshing"
        case .finished: "Updated"
        case .failed: "Refresh failed"
        case .idle: ""
        }
    }

    private var empty: some View {
        ContentUnavailableView {
            Label(courseFilter.isEmpty ? "No Tasks" : "No Results", systemImage: "checklist")
        } description: {
            Text(courseFilter.isEmpty ? "Add a task to get started." : "No tasks for this class.")
        }
    }

    /// Sorting stays in a menu; the class filter is the chips.
    @ToolbarContentBuilder private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .topBarTrailing) {
            Menu {
                Picker("Sort By", selection: $sort) {
                    ForEach(TaskSort.allCases.filter { $0 != .completion }, id: \.self) { sort in
                        Text(sort.label).tag(sort)
                    }
                }
                .pickerStyle(.inline)
                Toggle("Reverse Order", isOn: $reversed)
            } label: {
                Label("Sort", systemImage: "arrow.up.arrow.down")
                    .foregroundStyle(Color.toolbarIcon)
            }
            .tint(Color.toolbarIcon)
        }
    }

    private func name(_ code: String) -> String {
        state.schedule.sections.first { $0.course == code }.map { $0.title.isEmpty ? code : $0.title } ?? code
    }

    // MARK: Ticking

    /// Ticking fills the circle at once; the row then leaves for the
    /// completed fold once the check has landed. Reduce Motion moves it at once.
    private func complete(_ task: StudentTask) {
        let finishing = !task.done
        if finishing { lastCompleted = task.id }
        guard finishing, !reduceMotion else {
            lingering.remove(task.id)
            withAnimation(reduceMotion ? nil : .snappy(duration: 0.25)) { state.toggle(taskID: task.id) }
            return
        }
        lingering.insert(task.id)
        state.toggle(taskID: task.id)
        Task {
            try? await Task.sleep(for: .milliseconds(450))
            withAnimation(.smooth(duration: 0.3)) { _ = lingering.remove(task.id) }
        }
    }

    // MARK: Grouping

    private struct Group {
        let id: String
        let title: String
        let items: [TaskItem]
        let detail: (TaskItem) -> String?
    }

    private func matching(_ task: StudentTask) -> Bool {
        let courses = state.taskCourses
        return courseFilter.isEmpty || (courseFilter == "__none"
            ? courses.course(for: task) == nil
            : courses.course(for: task) == courseFilter)
    }

    private func items(_ tasks: [StudentTask]) -> [TaskItem] {
        let courses = state.taskCourses
        let schedule = state.schedule
        let zone = state.timeZone
        let today = state.today
        return tasks.map { TaskItem(task: $0, colorHex: courses.color(for: $0, in: schedule), zone: zone, today: today) }
    }

    private func finished() -> [TaskItem] {
        let done = state.library.tasks.filter { $0.done && !lingering.contains($0.id) && matching($0) }
        return items(TaskSort.completion.ordered(done, reversed: false) { state.taskCourses.course(for: $0) })
    }

    private func groups() -> [Group] {
        let open = state.library.tasks.filter { (!$0.done || lingering.contains($0.id)) && matching($0) }
        let ordered = items(sort.ordered(open, reversed: reversed) { state.taskCourses.course(for: $0) })
        let today = state.today
        guard sort == .due, !reversed else {
            return ordered.isEmpty ? [] : [Group(id: "all", title: "", items: ordered) { $0.due(from: today) }]
        }
        var buckets: [String: [TaskItem]] = [:]
        for item in ordered {
            let key: String
            if let date = item.date {
                switch date.day - today.day {
                case ..<0: key = "Overdue"
                case 0: key = "Today"
                case 1: key = "Tomorrow"
                case 2...6: key = "This Week"
                default: key = "Later"
                }
            } else {
                key = "No Due Date"
            }
            buckets[key, default: []].append(item)
        }
        // A heading that names the day leaves the row its time; one that
        // names a span leaves the day too.
        let order: [(String, (TaskItem) -> String?)] = [
            ("Overdue", { $0.date?.monthDay }),
            ("Today", { $0.time }),
            ("Tomorrow", { $0.time }),
            ("This Week", { $0.due(from: today) }),
            ("Later", { $0.due(from: today) }),
            ("No Due Date", { _ in nil }),
        ]
        return order.compactMap { title, detail in
            buckets[title].map { Group(id: title, title: title, items: $0, detail: detail) }
        }
    }
}

#if DEBUG
#Preview("Tasks") {
    NavigationStack { TasksView() }
        .environment(AppState.preview())
        .tint(.brand)
}
#endif
