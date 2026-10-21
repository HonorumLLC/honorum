import SwiftUI

/// Tasks matching a search, in a native list: open matches by due date,
/// then finished ones.
struct TaskSearchResults: View {
    @Environment(AppState.self) private var state
    let query: String
    let onOpen: (StudentTask) -> Void

    var body: some View {
        let today = state.today
        let found = matches
        List {
            if query.trimmingCharacters(in: .whitespaces).isEmpty {
                // Nothing typed yet.
            } else if found.isEmpty {
                ContentUnavailableView.search(text: query)
                    .listRowBackground(Color.clear)
            } else {
                section("Tasks", found.filter { !$0.done }, today: today)
                section("Completed", found.filter(\.done), today: today)
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color.surface.ignoresSafeArea())
        .scrollDismissesKeyboard(.immediately)
        .accessibilityIdentifier("search-results")
    }

    @ViewBuilder private func section(_ title: String, _ items: [TaskItem], today: CivilDate) -> some View {
        if !items.isEmpty {
            Section {
                ForEach(items) { item in
                    TaskLine(item: item, detail: item.due(from: today), detailIsUrgent: item.overdue,
                             onToggle: { state.toggle(taskID: item.id) }, onOpen: { onOpen(item.task) })
                        .listRowInsets(EdgeInsets())
                        .listRowBackground(Color.card)
                }
            } header: {
                ListHeader(title)
            }
        }
    }

    private func items(_ tasks: [StudentTask]) -> [TaskItem] {
        let courses = state.taskCourses
        return tasks.map {
            TaskItem(task: $0, colorHex: courses.color(for: $0, in: state.schedule), zone: state.timeZone, today: state.today)
        }
        .sorted { ($0.date?.day ?? .max, $0.minute) < ($1.date?.day ?? .max, $1.minute) }
    }

    private var matches: [TaskItem] {
        let needle = query.trimmingCharacters(in: .whitespaces).lowercased()
        let courses = state.taskCourses
        return items(state.library.tasks.filter { task in
            task.title.lowercased().contains(needle) || task.course.lowercased().contains(needle)
                || (courses.course(for: task).map { code in
                    state.schedule.sections.first { $0.course == code }?.title.lowercased().contains(needle) ?? false
                } ?? false)
        })
    }
}

/// Home's search and New button. On iOS 26 both sit in the bottom toolbar:
/// search on the left becomes the system's field on the keyboard, and Home's
/// sections give way to the results in place; New stays on the right, where
/// the thumb rests. Before iOS 26 the floating bar opens search in a sheet.
struct HomeSearch: ViewModifier {
    let compose: () -> Void
    let edit: (StudentTask) -> Void
    @State private var query = ""
    @State private var searching = false

    func body(content: Content) -> some View {
        if #available(iOS 26.0, *) {
            SearchSwap(query: query, edit: edit) { content }
                .searchable(text: $query, prompt: "Search tasks")
                .searchToolbarBehavior(.minimize)
                // The date and Account stay up while searching, so opening
                // and closing the search moves nothing but the field and the
                // keyboard.
                .searchPresentationToolbarBehavior(.avoidHidingContent)
                .toolbar {
                    DefaultToolbarItem(kind: .search, placement: .bottomBar)
                    ToolbarSpacer(.flexible, placement: .bottomBar)
                    ToolbarItem(placement: .bottomBar) {
                        Button(action: compose) {
                            // A plain view rather than a Label, which a
                            // bottom bar draws as its icon alone.
                            HStack(spacing: 6) {
                                Image(systemName: "plus")
                                Text("New")
                            }
                            .font(.body.weight(.semibold))
                            .padding(.horizontal, 8)
                        }
                        .buttonStyle(.glassProminent)
                        .tint(.brand)
                        .accessibilityLabel("New")
                        .accessibilityIdentifier("addButton")
                    }
                }
        } else {
            content
                .actionBar(onNew: compose, onSearch: { searching = true })
                .sheet(isPresented: $searching) { TaskSearchSheet() }
        }
    }
}

/// Home's sections, or the results once a search is typed. Home already
/// shows what is due soon, so it stays while the field is empty and opening
/// the search moves only the field and the keyboard; the results replace it
/// as the first letter is typed, and ending the search clears the text and
/// brings Home back.
private struct SearchSwap<Content: View>: View {
    let query: String
    let edit: (StudentTask) -> Void
    @ViewBuilder var content: Content

    var body: some View {
        if query.trimmingCharacters(in: .whitespaces).isEmpty {
            content
        } else {
            TaskSearchResults(query: query, onOpen: edit)
        }
    }
}

/// Search in a sheet, before iOS 26: the field is active as the sheet
/// rises, and ending the search closes it.
struct TaskSearchSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""
    @State private var active = true
    @State private var editing: StudentTask?

    var body: some View {
        NavigationStack {
            TaskSearchResults(query: query) { editing = $0 }
                .navigationTitle("Search Tasks")
                .navigationBarTitleDisplayMode(.inline)
                .searchable(text: $query, isPresented: $active,
                            placement: .navigationBarDrawer(displayMode: .always), prompt: "Search tasks")
                .onChange(of: active) { _, active in if !active { dismiss() } }
                .sheet(item: $editing) { TaskEditor(task: $0) }
        }
    }
}
