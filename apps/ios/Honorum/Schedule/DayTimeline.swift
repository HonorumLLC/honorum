import SwiftUI

/// The hour rules and their gutter labels, drawn once behind every day column.
struct HourGrid: View {
    /// The label column, which grows with the reader's text size up to a cap
    /// that keeps most of the width for the day.
    static var gutter: CGFloat { min(UIFontMetrics(forTextStyle: .footnote).scaledValue(for: 60), 96) }
    /// How far an hour rule rests below the chrome when the timeline settles
    /// on it, so its label clears the bar above.
    static let labelClearance: CGFloat = 14

    let hourHeight: CGFloat
    /// Campus minute of the current time, when the visible day is today.
    var now: Int?
    /// The day view leaves the hours unruled, so its cards stand on the
    /// surface; the week view keeps rules to read across seven columns.
    var rules = true

    @Environment(\.displayScale) private var displayScale
    @ScaledMetric(relativeTo: .footnote) private var labelHeight = 16.0

    var body: some View {
        ZStack(alignment: .topLeading) {
            ForEach(0...24, id: \.self) { hour in
                if rules { rule(hour) }
                if hour > 0, hour < 24, !obscured(hour) { label(hour) }
            }
            if let now { nowLine(now) }
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
    }

    /// The now pill sits where an hour label would, so that label steps
    /// aside, the way Calendar hides the hour nearest the current time.
    private func obscured(_ hour: Int) -> Bool {
        guard let now else { return false }
        let reach = Int((labelHeight / hourHeight * 60).rounded(.up)) + 2
        return abs(now - hour * 60) < reach
    }

    private func rule(_ hour: Int) -> some View {
        Rectangle()
            .fill(Color.hairline)
            .frame(height: 1 / displayScale)
            .padding(.leading, Self.gutter)
            .offset(y: CGFloat(hour) * hourHeight)
    }

    private func label(_ hour: Int) -> some View {
        let parts = Clock.hourParts(hour)
        return HStack(alignment: .firstTextBaseline, spacing: 2) {
            Text(parts.0).font(.footnote)
            if !parts.1.isEmpty {
                Text(parts.1).font(.caption2)
            }
        }
        .lineLimit(1)
        .minimumScaleFactor(0.6)
        .foregroundStyle(Color.inkMuted)
        .frame(width: Self.gutter - 10, height: labelHeight, alignment: .trailing)
        .offset(y: CGFloat(hour) * hourHeight - labelHeight / 2)
    }

    private func nowLine(_ minute: Int) -> some View {
        HStack(spacing: 0) {
            Text(Clock.time(minute))
                .font(.caption2.weight(.semibold))
                .foregroundStyle(Color.onNowLine)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(Capsule().fill(Color.nowLine))
            Rectangle().fill(Color.nowLine).frame(height: 1.5)
        }
        .padding(.leading, 8)
        .offset(y: CGFloat(minute) / 60 * hourHeight - labelHeight / 2 - 1)
    }
}

/// One day's blocks, positioned against the shared hour grid.
struct DayColumn: View {
    @AppStorage("tasksAtDueTime") private var tasksAtDueTime = false
    @Environment(AppState.self) private var state
    /// Below this height a block shows its title alone; it grows with the text.
    @ScaledMetric(relativeTo: .subheadline) private var compactHeight = 48.0
    let date: CivilDate
    let hourHeight: CGFloat
    let conflicts: Set<String>
    var onOpen: (Occurrence) -> Void

    /// Breathing room either side of a block, so a card never touches the
    /// screen edge or the hour gutter.
    private static let inset: CGFloat = 6

    var body: some View {
        let blocks = Occurrences.layOut((state.occurrences(on: date) + (tasksAtDueTime ? state.library.taskOccurrences(on: date, in: state.timeZone) : [])).sorted { ($0.start, $0.end) < ($1.start, $1.end) })
        GeometryReader { geometry in
            ZStack(alignment: .topLeading) {
                ForEach(blocks) { block in
                    let width = (geometry.size.width - Self.inset * 2) / CGFloat(block.columns)
                    Button { onOpen(block) } label: {
                        EventBlock(
                            block: block,
                            conflicting: isConflicting(block, conflicts),
                            compact: height(of: block) < compactHeight)
                    }
                    .buttonStyle(.plain)
                    .frame(width: max(width - 2, 24), height: max(height(of: block) - 2, 18))
                    .offset(
                        x: Self.inset + width * CGFloat(block.column),
                        y: CGFloat(block.start) / 60 * hourHeight + 1)
                }
            }
        }
    }

    private func height(of block: Occurrence) -> CGFloat {
        CGFloat(block.end - block.start) / 60 * hourHeight
    }

    private func isConflicting(_ block: Occurrence, _ conflicts: Set<String>) -> Bool {
        if case .section(let id) = block.kind { return conflicts.contains(id) }
        return false
    }
}

/// A class meeting or personal event: a pastel card with dark ink, the way the
/// web planner draws it, sized to whatever the hour grid allows. A class leads
/// with its name and carries its code at the trailing edge; the time and room
/// share the line beneath when the card is tall enough.
struct EventBlock: View {
    let block: Occurrence
    var conflicting = false
    var compact = false

    /// The block's corner, shared by the fill and the outlines drawn on it.
    static let radius: CGFloat = 14

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Self.radius, style: .continuous)
        VStack(alignment: .leading, spacing: 2) {
            // The code steps aside when a narrow week column has no room for it.
            ViewThatFits(in: .horizontal) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    name
                    Spacer(minLength: 0)
                    if let tag {
                        Text(tag)
                            .font(.caption.weight(.medium))
                            .foregroundStyle(Color.eventInkSecondary)
                            .lineLimit(1)
                            .fixedSize()
                    }
                }
                name.frame(maxWidth: .infinity, alignment: .leading)
            }
            if !compact {
                Text(details)
                    .font(.caption)
                    .foregroundStyle(Color.eventInkSecondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .foregroundStyle(Color.eventInk)
        .padding(.horizontal, compact ? 8 : 12)
        .padding(.vertical, compact ? 3 : 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background { shape.fill(CoursePalette.fill(block.colorHex)) }
        .overlay {
            if case .exam = block.kind {
                shape.strokeBorder(Color.eventInkSecondary, lineWidth: 1)
            }
            if conflicting {
                shape.strokeBorder(Color.nowLine, lineWidth: 1.5)
            }
        }
        .contentShape(.rect)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(accessibilityDescription)
    }

    private var name: some View {
        Text(heading)
            .strikethrough(block.completed)
            .font(.subheadline.weight(.semibold))
            .lineLimit(1)
    }

    /// A class goes by its course title, with the code as the tag; anything
    /// else goes by its own title, tagged with its class when it has one.
    private var heading: String {
        if case .section = block.kind, !block.subtitle.isEmpty { return block.subtitle }
        return block.title
    }

    private var tag: String? {
        if case .section = block.kind { return block.subtitle.isEmpty ? nil : block.title }
        if case .exam = block.kind { return nil }
        return block.subtitle.isEmpty || block.subtitle == block.title ? nil : block.subtitle
    }

    /// Where, and until when, as Home's blocks put it.
    private var details: String {
        let until = "until \(Clock.time(block.end))"
        return block.location.isEmpty ? until : "\(block.location) · \(until)"
    }

    /// Everything the card says, plus what it only shows: a completed task,
    /// an exam sitting, and a time conflict.
    private var accessibilityDescription: String {
        var parts = [block.title]
        if !block.subtitle.isEmpty, block.subtitle != block.title { parts.append(block.subtitle) }
        parts.append(Clock.range(block.start, block.end))
        if !block.location.isEmpty { parts.append(block.location) }
        if case .exam = block.kind { parts.append("Exam") }
        if block.completed { parts.append("Completed") }
        if conflicting { parts.append("Conflicts with another class") }
        return parts.joined(separator: ". ")
    }
}

/// The whole week at once, for iPad and other regular-width layouts. The week
/// strip above it heads the columns, offset by the hour gutter so each day
/// sits under its number.
struct WeekTimeline: View {
    @Environment(AppState.self) private var state
    let week: CivilDate
    let hourHeight: CGFloat
    var onOpen: (Occurrence) -> Void

    @State private var vertical = ScrollPosition(y: ScheduleView.morning)
    @AppStorage("tasksAtDueTime") private var tasksAtDueTime = false

    var body: some View {
        let conflicts = state.conflicts
        return ScrollView(.vertical) {
            ZStack(alignment: .topLeading) {
                HourGrid(hourHeight: hourHeight, now: nowMinute)
                HStack(spacing: 0) {
                    Color.clear.frame(width: HourGrid.gutter)
                    ForEach(0..<7, id: \.self) { offset in
                        DayColumn(
                            date: week.adding(days: offset), hourHeight: hourHeight,
                            conflicts: conflicts, onOpen: onOpen
                        )
                        .overlay(alignment: .leading) {
                            if offset > 0 {
                                Rectangle().fill(Color.hairline).frame(width: 0.5)
                            }
                        }
                    }
                }
            }
            .frame(height: hourHeight * 24)
        }
        .background(Color.content)
        .scrollPosition($vertical)
        .hourJumpScrolling(hourHeight: hourHeight)
        .scrollIndicators(.hidden)
        .plannerTopBar {
            if !tasksAtDueTime {
                HStack(alignment: .center, spacing: 0) {
                    Text("Due").font(.caption).foregroundStyle(Color.inkMuted).frame(width: HourGrid.gutter)
                    ForEach(0..<7, id: \.self) { offset in
                        let date = week.adding(days: offset)
                        // Only a day with deadlines gets a shelf, so an empty week keeps a thin row.
                        Group { if !state.tasks(on: date).isEmpty { DueTasks(date: date) } }
                            .frame(maxWidth: .infinity)
                    }
                }
                .padding(.vertical, 6)
                .dueBarSurface()
            }
        }
    }

    private var nowMinute: Int? {
        let clock = state.clock
        let start = week.day
        return (start...(start + 6)).contains(clock.date.day) ? clock.minute : nil
    }
}


/// One horizontal shelf keeps deadlines within reach without shrinking the day.
/// The shelf outlasts its date: when the date changes, the chips inside
/// crossfade and the shelf returns to its leading edge, but the scroll view
/// itself stays, empty on a day with nothing due. The day planner relies on
/// that, because mounting a scroll view under a finger disturbs its pager.
struct DueTasks: View {
    @Environment(AppState.self) private var state
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let date: CivilDate
    var trailingInset: CGFloat = 0
    @State private var editing: StudentTask?
    @State private var position = ScrollPosition(edge: .leading)
    @ScaledMetric(relativeTo: .subheadline) private var stripHeight = 44.0

    var body: some View {
        let due = state.tasks(on: date)
        // What still needs doing comes first; finished tasks recede to the end.
        let tasks = due.filter { !$0.done } + due.filter(\.done)
        ScrollView(.horizontal) {
            // Overlaid, so the day leaving fades beneath the one arriving.
            ZStack(alignment: .leading) {
                HStack(spacing: 8) {
                    ForEach(tasks) { chip($0) }
                }
                .scrollTargetLayout()
                .animation(reduceMotion ? nil : .snappy(duration: 0.3), value: tasks.map(\.id))
                .id(date)
                .transition(.opacity)
            }
        }
        .contentMargins(.trailing, trailingInset, for: .scrollContent)
        .scrollPosition($position)
        .scrollTargetBehavior(.viewAligned)
        .scrollIndicators(.hidden)
        .scrollBounceBehavior(.basedOnSize)
        .frame(height: stripHeight)
        .accessibilityIdentifier("due-tasks-" + date.iso)
        .accessibilityHidden(tasks.isEmpty)
        .onChange(of: date) { position.scrollTo(edge: .leading) }
        .animation(PagerProgress.chromeAnimation(reduceMotion: reduceMotion), value: date)
        .sheet(item: $editing) { TaskEditor(task: $0) }
    }

    /// A due task as a capsule in Home's style: its tick in the class's
    /// tone, its title, and its time when that is not the end of the day.
    private func chip(_ task: StudentTask) -> some View {
        let item = TaskItem(task: task, colorHex: state.taskCourses.color(for: task, in: state.schedule),
                            zone: state.timeZone, today: state.today)
        return Button { editing = task } label: {
            HStack(spacing: 8) {
                TaskTick(colorHex: item.colorHex, done: task.done, size: 18)
                Text(task.title)
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(task.done ? Color.inkMuted : Color.ink)
                    .strikethrough(task.done, color: Color.inkMuted)
                    .lineLimit(1)
                    .frame(maxWidth: 180, alignment: .leading)
                if let time = item.time {
                    Text(time)
                        .font(.caption)
                        .monospacedDigit()
                        .foregroundStyle(Color.inkMuted)
                        .fixedSize()
                }
            }
            .padding(.leading, 8)
            .padding(.trailing, 14)
            .frame(minHeight: stripHeight - 10)
            .background(Color.card, in: .capsule)
            .overlay { Capsule().strokeBorder(Color.hairline, lineWidth: 1) }
            .contentShape(.capsule)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(task.title)
        .accessibilityValue(task.done ? "Completed" : "")
        .accessibilityHint("Edit task")
    }
}
