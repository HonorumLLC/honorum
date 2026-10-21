import SwiftUI

// The task pieces Home and the task list share: the tick circle, the row, a
// day's card grouped under its deadlines, and a card of tasks across days.

/// A task as the lists show it: its class color and its deadline in campus
/// date and minute.
struct TaskItem: Identifiable, Hashable {
    let task: StudentTask
    let colorHex: String
    let date: CivilDate?
    /// Campus minutes past midnight of the deadline.
    let minute: Int
    let overdue: Bool

    var id: String { task.id }
    var title: String { task.title.isEmpty ? "Untitled" : task.title }
    var done: Bool { task.done }

    init(task: StudentTask, colorHex: String, zone: TimeZone, today: CivilDate) {
        let due = task.dueDate.map { CampusClock($0, in: zone) }
        self.task = task
        self.colorHex = colorHex
        date = due?.date
        minute = due?.minute ?? 0
        overdue = !task.done && (due?.date ?? today) < today
    }

    /// The deadline's time, unless it is the end of the day most work carries.
    var time: String? {
        date == nil || minute == 23 * 60 + 59 ? nil : Clock.time(minute)
    }

    /// The deadline's day, near days by name: "Tomorrow", "Fri", "Oct 15".
    func day(from today: CivilDate) -> String? {
        guard let date else { return nil }
        let delta = date.day - today.day
        if delta == 0 { return "Today" }
        if delta == 1 { return "Tomorrow" }
        if delta == -1 { return "Yesterday" }
        if delta > 1 && delta < 7 { return date.formatted(.dateTime.weekday(.abbreviated)) }
        return date.monthDay
    }

    /// The day and, when it is not the end of the day, the time.
    func due(from today: CivilDate) -> String? {
        guard let day = day(from: today) else { return nil }
        return time.map { "\(day), \($0)" } ?? day
    }
}

/// A task's tick: an open ring in the class's deeper tone, filled and
/// checked once done.
struct TaskTick: View {
    let colorHex: String
    let done: Bool
    var size: CGFloat = 24

    var body: some View {
        ZStack {
            if done {
                Circle().fill(CoursePalette.tone(colorHex))
                Image(systemName: "checkmark")
                    .font(.system(size: size * 0.42, weight: .heavy))
                    .foregroundStyle(.white)
            } else {
                Circle().fill(Color.card)
                Circle().strokeBorder(CoursePalette.tone(colorHex), lineWidth: 3.5)
            }
        }
        .frame(width: size, height: size)
    }
}

/// A task: its tick, in its class's color, its title, and, where its card
/// doesn't already say, when it is due. The tick completes the task; the rest
/// of the row opens it.
struct TaskLine: View {
    let item: TaskItem
    var detail: String?
    var detailIsUrgent = false
    let onToggle: () -> Void
    let onOpen: () -> Void

    var body: some View {
        HStack(spacing: 0) {
            Button(action: onToggle) {
                TaskTick(colorHex: item.colorHex, done: item.done)
                    .frame(width: 52, height: 44)
                    .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(item.done ? "Mark Not Done" : "Mark Done")
            .accessibilityValue(item.title)
            .accessibilityIdentifier("tick-\(item.id)")
            Button(action: onOpen) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(item.title)
                        .font(.body)
                        .foregroundStyle(item.done ? Color.inkMuted : Color.ink)
                        .strikethrough(item.done, color: Color.inkMuted)
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)
                    Spacer(minLength: 0)
                    if let detail {
                        Text(detail)
                            .font(.subheadline.weight(detailIsUrgent ? .semibold : .regular))
                            .monospacedDigit()
                            .foregroundStyle(detailIsUrgent ? Color.danger : Color.inkMuted)
                            .lineLimit(1)
                    }
                }
                .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                .padding(.trailing, 16)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .accessibilityHint("Opens the task")
            .accessibilityIdentifier("task-\(item.id)")
        }
        .padding(.vertical, 2)
    }
}

/// A day's tasks, grouped under the deadlines they share: most fall under
/// 11:59 PM, and anything due earlier gets its own heading above. Overdue
/// work leads. Finished tasks fold into one row at the end.
struct DeadlineCard: View {
    let items: [TaskItem]
    var empty = "Nothing due"
    let onToggle: (TaskItem) -> Void
    let onOpen: (TaskItem) -> Void
    @State private var showsDone = false

    private struct Deadline: Identifiable {
        let id: Int
        let overdue: Bool
        let items: [TaskItem]
    }

    var body: some View {
        let open = items.filter { !$0.done }
        let done = items.filter(\.done)
        let overdue = open.filter(\.overdue)
        let current = open.filter { !$0.overdue }
        let groups = (overdue.isEmpty ? [] : [Deadline(id: -1, overdue: true, items: overdue)])
            + Array(Set(current.map(\.minute))).sorted().map { minute in
                Deadline(id: minute, overdue: false, items: current.filter { $0.minute == minute })
            }
        VStack(spacing: 0) {
            if items.isEmpty {
                Text(empty)
                    .font(.body)
                    .foregroundStyle(Color.inkMuted)
                    .frame(maxWidth: .infinity, minHeight: 52, alignment: .leading)
                    .padding(.horizontal, 16)
            }
            ForEach(Array(groups.enumerated()), id: \.element.id) { index, group in
                if index > 0 { RowRule() }
                heading(group)
                ForEach(Array(group.items.enumerated()), id: \.element.id) { row, item in
                    if row > 0 { RowRule(inset: 52) }
                    TaskLine(item: item, onToggle: { onToggle(item) }, onOpen: { onOpen(item) })
                }
            }
            if !done.isEmpty {
                if !open.isEmpty { RowRule() }
                CompletedFold(count: done.count, open: $showsDone)
                if showsDone {
                    ForEach(done) { item in
                        RowRule(inset: 52)
                        TaskLine(item: item, onToggle: { onToggle(item) }, onOpen: { onOpen(item) })
                    }
                }
            }
        }
        .card(padding: 0)
    }

    private func heading(_ group: Deadline) -> some View {
        Text(group.overdue ? "Overdue" : Clock.time(group.id))
            .font(.footnote.weight(.semibold))
            .monospacedDigit()
            .foregroundStyle(group.overdue ? Color.danger : Color.inkMuted)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16)
            .padding(.top, 12)
            .padding(.bottom, -3)
            .accessibilityAddTraits(.isHeader)
    }
}

/// "N completed", a row that folds finished work away.
struct CompletedFold: View {
    let count: Int
    @Binding var open: Bool

    var body: some View {
        Button {
            withAnimation(.smooth(duration: 0.3)) { open.toggle() }
        } label: {
            HStack {
                Text("\(count) completed")
                    .font(.subheadline)
                    .foregroundStyle(Color.inkMuted)
                Spacer()
                Image(systemName: "chevron.down")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(Color.inkMuted)
                    .rotationEffect(.degrees(open ? 180 : 0))
            }
            .padding(.horizontal, 16)
            .frame(minHeight: 44)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(open ? "Hide \(count) completed" : "Show \(count) completed")
        .accessibilityIdentifier("completedToggle")
    }
}

/// Tasks across days as one card, each with its day (and its time, when it
/// isn't the end of the day) on the right.
struct TaskListCard: View {
    let items: [TaskItem]
    let today: CivilDate
    var empty = "Nothing coming up"
    /// What a row says on the right; by default its day and time.
    var detail: ((TaskItem) -> String?)?
    let onToggle: (TaskItem) -> Void
    let onOpen: (TaskItem) -> Void
    var onAll: (() -> Void)?

    var body: some View {
        VStack(spacing: 0) {
            if items.isEmpty {
                Text(empty)
                    .font(.body)
                    .foregroundStyle(Color.inkMuted)
                    .frame(maxWidth: .infinity, minHeight: 52, alignment: .leading)
                    .padding(.horizontal, 16)
            }
            ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                if index > 0 { RowRule(inset: 52) }
                TaskLine(item: item, detail: detail.map { $0(item) } ?? item.due(from: today),
                         detailIsUrgent: item.overdue,
                         onToggle: { onToggle(item) }, onOpen: { onOpen(item) })
            }
            if let onAll {
                RowRule()
                Button(action: onAll) {
                    HStack {
                        Text("All Tasks")
                            .font(.subheadline.weight(.medium))
                            .foregroundStyle(Color.brandInk)
                        Spacer()
                        Image(systemName: "chevron.right")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(Color.inkMuted)
                    }
                    .padding(.horizontal, 16)
                    .frame(minHeight: 44)
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("allTasks")
            }
        }
        .padding(.vertical, 2)
        .card(padding: 0)
    }
}
