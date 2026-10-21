import SwiftUI

/// What the New button makes: a task, or an event on the calendar.
enum NewKind: String, CaseIterable, Identifiable {
    case task = "Task"
    case event = "Event"

    var id: String { rawValue }
}

/// The sheet behind every New button: one place to add a task or an event,
/// with a switch between the two at the top. A title typed before switching
/// comes along.
struct NewItemSheet: View {
    @State var kind: NewKind
    /// The event's day and start, from the screen that asked.
    let event: PersonalEvent
    /// A task to start from, for screenshots of a filled-in sheet.
    var task = StudentTask()
    @State private var title = ""

    init(kind: NewKind, event: PersonalEvent, task: StudentTask = StudentTask()) {
        _kind = State(initialValue: kind)
        self.event = event
        self.task = task
        _title = State(initialValue: task.title.isEmpty ? event.title : task.title)
    }

    var body: some View {
        let sample = !task.title.isEmpty
        switch kind {
        case .task:
            TaskEditor(task: draftTask, isNew: true, kind: $kind, focusesTitle: !sample) { title = $0 }
        case .event:
            EventEditor(event: draftEvent, isNew: true, kind: $kind, focusesTitle: !sample) { title = $0 }
        }
    }

    private var draftTask: StudentTask {
        var draft = task
        draft.title = title
        return draft
    }

    private var draftEvent: PersonalEvent {
        var draft = event
        draft.title = title
        return draft
    }
}

/// The Task and Event switch, in the sheet's title position.
struct KindSwitch: View {
    @Binding var kind: NewKind

    var body: some View {
        Picker("Kind", selection: $kind) {
            ForEach(NewKind.allCases) { Text($0.rawValue).tag($0) }
        }
        .pickerStyle(.segmented)
        .frame(width: 180)
        .accessibilityIdentifier("newKind")
    }
}

// MARK: Editor pieces

/// An editor's card with a small label over its content.
struct EditorCard<Content: View>: View {
    var label: String?
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let label {
                Text(label)
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(Color.inkMuted)
            }
            content
        }
        .card(padding: 16)
    }
}

/// A labeled control on one line of an editor card.
struct EditorRow<Control: View>: View {
    let label: String
    @ViewBuilder var control: Control

    var body: some View {
        HStack {
            Text(label).foregroundStyle(Color.ink)
            Spacer(minLength: 8)
            control
        }
        .frame(minHeight: 40)
    }
}

/// The schedule's classes as chips in their colors, with None first.
struct ClassChips: View {
    @Environment(AppState.self) private var state
    @Binding var code: String
    var none = "None"
    /// Codes to offer beyond the schedule's, like an imported task's own.
    var extra: [String] = []

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView(.horizontal) {
                HStack(spacing: 8) {
                    Chip(title: none, selected: code.isEmpty) { code = "" }
                        .id("")
                    ForEach(codes, id: \.self) { item in
                        Chip(title: name(item), hex: state.schedule.color(for: item), selected: code == item) { code = item }
                            .accessibilityLabel(item)
                            .id(item)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 1)
            }
            .scrollIndicators(.hidden)
            .padding(.horizontal, -16)
            // A chosen class further along the row is brought into view.
            .onAppear { if !code.isEmpty { proxy.scrollTo(code, anchor: .center) } }
        }
    }

    private var codes: [String] {
        var list: [String] = []
        for section in state.schedule.sections where !list.contains(section.course) { list.append(section.course) }
        for item in extra where !item.isEmpty && !list.contains(item) { list.append(item) }
        return list
    }

    private func name(_ code: String) -> String {
        state.schedule.sections.first { $0.course == code }.map { $0.title.isEmpty ? code : $0.title } ?? code
    }
}
