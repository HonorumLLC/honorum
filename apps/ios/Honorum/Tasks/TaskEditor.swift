import SwiftUI

/// Adds or edits one task: its name in large type, its class as a row of
/// colored chips, and its deadline as near days to tap or any date to pick.
struct TaskEditor: View {
    @Environment(AppState.self) private var state
    @Environment(\.dismiss) private var dismiss

    @State private var draft: StudentTask
    @State private var hasDue: Bool
    @State private var due: Date
    @State private var confirmingDelete = false
    @FocusState private var titleFocused: Bool
    private let isNew: Bool
    /// The Task and Event switch, when the sheet is adding something new.
    private var kind: Binding<NewKind>?
    private let focusesTitle: Bool
    private var onTitle: ((String) -> Void)?

    init(task: StudentTask, isNew: Bool? = nil, kind: Binding<NewKind>? = nil, focusesTitle: Bool = true,
         onTitle: ((String) -> Void)? = nil) {
        self.focusesTitle = focusesTitle
        _draft = State(initialValue: task)
        _hasDue = State(initialValue: task.dueDate != nil)
        _due = State(initialValue: task.dueDate ?? Date())
        self.isNew = isNew ?? task.title.isEmpty
        self.kind = kind
        self.onTitle = onTitle
    }

    /// The end of the campus day, the deadline most assignments carry.
    private static func endOfDay(_ date: CivilDate, in zone: TimeZone) -> Date {
        date.instant(minute: 23 * 60 + 59, in: zone)
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 12) {
                    TextField("Task name", text: $draft.title, axis: .vertical)
                        .font(.display(22, relativeTo: .title2))
                        .foregroundStyle(Color.ink)
                        .lineLimit(1...4)
                        .focused($titleFocused)
                        .accessibilityIdentifier("Title")
                        .card(padding: 18)
                    EditorCard(label: "Class") {
                        ClassChips(code: courseBinding, extra: [draft.course])
                    }
                    dueCard
                    EditorCard(label: "Notes") {
                        TextField("Add notes", text: $draft.notes, axis: .vertical)
                            .lineLimit(2...8)
                    }
                    if draft.source != .manual { sourceCard }
                    if !isNew {
                        EditorCard {
                            Toggle("Completed", isOn: $draft.done)
                                .tint(.brand)
                        }
                        Button("Delete Task", role: .destructive) { confirmingDelete = true }
                            .buttonStyle(.softPill)
                            .foregroundStyle(Color.danger)
                            .padding(.top, 6)
                    }
                }
                .padding(16)
            }
            .scrollDismissesKeyboard(.interactively)
            .background(Color.surface.ignoresSafeArea())
            .navigationTitle(isNew ? "New Task" : "Edit Task")
            .navigationBarTitleDisplayMode(.inline)
            // Deadlines are campus times, wherever the phone is.
            .environment(\.timeZone, state.timeZone)
            .onAppear {
                if draft.dueDate == nil { due = Self.endOfDay(state.today, in: state.timeZone) }
                if isNew && focusesTitle { titleFocused = true }
            }
            .onChange(of: draft.title) { _, title in onTitle?(title) }
            .confirmationDialog("Delete this task?", isPresented: $confirmingDelete, titleVisibility: .visible) {
                Button("Delete Task", role: .destructive) {
                    state.removeTask(id: draft.id)
                    dismiss()
                }
            } message: {
                Text(draft.title)
            }
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .tint(Color.toolbarIcon)
                }
                if let kind {
                    ToolbarItem(placement: .principal) { KindSwitch(kind: kind) }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(isNew ? "Add" : "Done", action: save)
                        .tint(Color.brand)
                        .disabled(draft.title.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
        }
    }

    /// Near days as chips, then the date and time themselves.
    private var dueCard: some View {
        EditorCard(label: "Due") {
            ScrollView(.horizontal) {
                HStack(spacing: 8) {
                    Chip(title: "No date", selected: !hasDue) {
                        withAnimation(.snappy) { hasDue = false }
                    }
                    ForEach(0..<6, id: \.self) { offset in
                        let date = state.today.adding(days: offset)
                        Chip(title: offset == 0 ? "Today" : offset == 1 ? "Tomorrow" : date.formatted(.dateTime.weekday(.abbreviated)),
                             selected: hasDue && CivilDate(due, in: state.timeZone) == date) {
                            withAnimation(.snappy) { choose(date) }
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 1)
            }
            .scrollIndicators(.hidden)
            .padding(.horizontal, -16)
            if hasDue {
                RowRule().padding(.vertical, 2)
                EditorRow(label: "Date") {
                    DatePicker("Date", selection: $due, displayedComponents: .date)
                        .labelsHidden()
                }
                EditorRow(label: "Time") {
                    DatePicker("Time", selection: $due, displayedComponents: .hourAndMinute)
                        .labelsHidden()
                }
            }
        }
    }

    private var sourceCard: some View {
        EditorCard(label: "From \(draft.source.rawValue.capitalized)") {
            if draft.sourceURL != nil {
                TaskSourceLink(task: draft) {
                    Label(draft.source == .canvas ? "Open in Canvas" : "Open Source",
                          systemImage: "arrow.up.right.square")
                        .font(.body.weight(.medium))
                        .foregroundStyle(Color.brandInk)
                        .frame(maxWidth: .infinity, minHeight: 40, alignment: .leading)
                }
            } else {
                Text("Imported").foregroundStyle(Color.inkSecondary)
            }
        }
    }

    /// A near day keeps the time already chosen.
    private func choose(_ date: CivilDate) {
        let minute = hasDue ? CampusClock(due, in: state.timeZone).minute : 23 * 60 + 59
        due = date.instant(minute: minute, in: state.timeZone)
        hasDue = true
    }

    private func save() {
        // An untouched date keeps the source's own text, which an import
        // compares with what the source says next.
        let chosen: Date? = hasDue ? due : nil
        if chosen != draft.dueDate { draft.due = chosen.map(ISO8601.string(from:)) ?? "" }
        state.save(task: draft)
        dismiss()
    }

    /// Choosing a class by hand marks the task so a later import leaves it be.
    private var courseBinding: Binding<String> {
        Binding(
            get: { draft.course },
            set: { value in
                if draft.source != .manual, value != draft.course { draft.courseEdited = true }
                draft.course = value
            })
    }
}
