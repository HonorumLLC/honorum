import SwiftUI

/// A shared vertical timeline with native horizontal day paging.
struct ScheduleView: View {
    @Environment(AppState.self) private var state
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// The timeline opens on the morning, the way a class day starts, rather
    /// than at midnight.
    @State private var vertical = ScrollPosition(y: ScheduleView.morning)
    @State private var editingTask: StudentTask?
    @State private var openedDate: CivilDate?
    @State private var editingEvent: PersonalEvent?
    /// An event the detail sheet asked to edit; the editor opens once the
    /// detail has gone, since two sheets cannot swap places mid-dismissal.
    @State private var pendingEdit: PersonalEvent?
    @State private var opened: Occurrence?
    @State private var managingSchedules = false
    @State private var sharing = false
    /// The stack's path, owned by the tab so that pushing the class search and
    /// popping a course are plain stack operations that never re-evaluate a
    /// presentation flag here.
    @Binding var routes: [ClassesRoute]
    /// The day the pager is showing, which the day heading and week strip
    /// follow while a swipe is under way.
    @State private var progress = PagerProgress()

    /// An hour of the timeline at the default text size, zoomed in so a class
    /// stands tall: about seven hours fill a phone screen.
    static let baseHourHeight = 80.0

    /// An hour of the timeline, which grows with the reader's text size so
    /// the hour labels and a class's title keep their room.
    @ScaledMetric(relativeTo: .footnote) private var hourHeight = ScheduleView.baseHourHeight

    /// Where the timeline opens: the 7 AM rule just under the chrome, with its
    /// label clear of the bar above it.
    static var morning: CGFloat {
        7 * UIFontMetrics(forTextStyle: .footnote).scaledValue(for: baseHourHeight) - HourGrid.labelClearance
    }

    /// The day the pager is showing most of, ahead of the committed selection.
    private var displayedDate: CivilDate {
        progress.displayedDay.map(CivilDate.init(day:)) ?? state.selectedDate
    }

    private var displayedSelection: Binding<CivilDate> {
        Binding(get: { displayedDate }, set: { state.selectedDate = $0 })
    }

    var body: some View {
        @Bindable var state = state
        Group {
            VStack(spacing: 0) {
                dayHeader
                if showsFullWeek {
                    WeekStrip(selected: displayedSelection, today: state.today,
                              leadingInset: HourGrid.gutter)
                    Divider()
                    WeekTimeline(
                        week: state.selectedDate.startOfWeek(), hourHeight: hourHeight,
                        onOpen: open)
                } else {
                    WeekStrip(selected: displayedSelection, today: state.today, card: true)
                        .padding(.horizontal)
                        .padding(.bottom, 8)
                    dayPager
                }
            }
            .background(Color.surface)
            // The day heads the content instead; the month stays the screen's
            // name for VoiceOver and the back button of pushed screens.
            .navigationTitle(displayedDate.monthTitle)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar(removing: .title)
            .toolbar { toolbar }
            .sheet(item: $editingTask) { TaskEditor(task: $0) }
            .sheet(item: $editingEvent) { EventEditor(event: $0) }
            .sheet(item: $opened, onDismiss: {
                if let pendingEdit {
                    editingEvent = pendingEdit
                    self.pendingEdit = nil
                }
            }) { occurrence in
                OccurrenceDetail(occurrence: occurrence, date: openedDate ?? state.selectedDate) {
                    pendingEdit = $0
                }
            }
            .sheet(isPresented: $managingSchedules) { SchedulesView() }
            .sheet(isPresented: $sharing) { ShareScheduleView() }
        }

    }

    private func open(_ occurrence: Occurrence) {
        if case .task(let id) = occurrence.kind {
            editingTask = state.library.tasks.first { $0.id == id }
        } else {
            openedDate = occurrence.date ?? state.selectedDate
            opened = occurrence
        }
    }

    private var showsFullWeek: Bool { sizeClass == .regular }

    // MARK: Pieces

    /// The day as the screen's heading: the weekday large with the date
    /// beside it on the same baseline, and a closure's note beneath when campus
    /// is closed. Swiping it changes the day as the pager does, and VoiceOver
    /// adjusts it a day at a time.
    private var dayHeader: some View {
        let date = displayedDate
        return VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(date.weekdayName)
                    .font(.display(30, relativeTo: .largeTitle))
                    .foregroundStyle(Color.ink)
                Text(date.monthDay)
                    .font(.display(20, relativeTo: .title3))
                    .foregroundStyle(Color.inkMuted)
            }
            .lineLimit(1)
            .minimumScaleFactor(0.7)
            if let closure = state.closures[date] {
                Text(closureNote(closure))
                    .font(.subheadline)
                    .foregroundStyle(Color.inkSecondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal)
        .padding(.top, 4)
        .padding(.bottom, 12)
        .background(Color.surface)
        .contentShape(.rect)
        .simultaneousGesture(daySwipe)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("selected-date")
        .accessibilityValue(date.iso)
        .accessibilityAdjustableAction { direction in
            switch direction {
            case .increment: state.selectedDate = displayedDate.adding(days: 1)
            case .decrement: state.selectedDate = displayedDate.adding(days: -1)
            @unknown default: break
            }
        }
    }

    private func closureNote(_ closure: Closure) -> String {
        if let after = closure.after, after > 0 {
            return "\(closure.name) · No classes from \(Clock.time(after))"
        }
        guard let letter = closure.follows?.first,
            let index = Weekdays.letters.firstIndex(of: letter)
        else { return closure.name }
        let position = Weekdays.letters.distance(from: Weekdays.letters.startIndex, to: index)
        return "\(closure.name) · \(Weekdays.names[position]) classes meet"
    }

    private var dayPager: some View {
        @Bindable var state = state
        return ScrollView(.vertical) {
            ZStack(alignment: .topLeading) {
                HourGrid(hourHeight: hourHeight, now: nowMinute, rules: false)
                DayPager(selected: $state.selectedDate, progress: progress, hourHeight: hourHeight,
                         conflicts: state.conflicts, onOpen: open)
                    .padding(.leading, HourGrid.gutter)
            }
            .frame(height: hourHeight * 24)
        }
        .background(Color.surface)
        .overlay { if isEmptyTerm { emptyPlanner } }
        .accessibilityIdentifier("day-timeline")
        .scrollPosition($vertical)
        .hourJumpScrolling(hourHeight: hourHeight)
        .scrollIndicators(.hidden)
    }

    private var daySwipe: some Gesture {
        DragGesture(minimumDistance: 20)
            .onEnded { value in
                guard abs(value.translation.width) > 50,
                    abs(value.translation.width) > abs(value.translation.height) * 2
                else { return }
                withAnimation(reduceMotion ? nil : .snappy(duration: 0.3)) {
                    state.selectedDate = displayedDate.adding(days: value.translation.width < 0 ? 1 : -1)
                }
            }
    }

    private var isEmptyTerm: Bool {
        state.schedule.sections.isEmpty && state.schedule.events.isEmpty && state.library.tasks.isEmpty
    }

    private var emptyPlanner: some View {
        ContentUnavailableView {
            Label("Nothing Planned Yet", systemImage: "books.vertical")
        } description: {
            Text("Add your classes, or an event with New.")
        } actions: {
            Button("Add Classes") { editClasses() }
                .buttonStyle(.pill)
                .fixedSize()
        }
        .background(Color.surface)
    }

    /// The now indicator belongs on today, and nowhere else.
    private var nowMinute: Int? {
        state.selectedDate == state.today ? state.clock.minute : nil
    }

    @ToolbarContentBuilder private var toolbar: some ToolbarContent {
        // Today is always here, the way Calendar keeps it: on today it scrolls
        // the timeline to now. A toolbar item that never changes cannot be
        // taller than its neighbours, cannot flicker as the pager passes
        // today, and never re-lays out the navigation bar under a finger.
        ToolbarItem(placement: .topBarLeading) {
            Button("Today", action: goToToday)
                .foregroundStyle(Color.toolbarIcon)
                .tint(Color.toolbarIcon)
        }
        // The week view has no pager, so its arrows live here and step a week.
        if showsFullWeek {
            ToolbarItem(placement: .topBarLeading) {
                Button("Previous Week", systemImage: "chevron.left") {
                    withAnimation(reduceMotion ? nil : .snappy(duration: 0.22)) {
                        state.selectedDate = state.selectedDate.adding(days: -7)
                    }
                }
                .tint(Color.toolbarIcon)
            }
            ToolbarItem(placement: .topBarLeading) {
                Button("Next Week", systemImage: "chevron.right") {
                    withAnimation(reduceMotion ? nil : .snappy(duration: 0.22)) {
                        state.selectedDate = state.selectedDate.adding(days: 7)
                    }
                }
                .tint(Color.toolbarIcon)
            }
        }
        ToolbarItem(placement: .topBarTrailing) {
            Menu {
                Button("Schedules", systemImage: "square.stack") { managingSchedules = true }
                Button("Share Schedule", systemImage: "square.and.arrow.up") { sharing = true }
            } label: {
                Label("Schedule", systemImage: "ellipsis")
                    .foregroundStyle(Color.toolbarIcon)
            }
            .tint(Color.toolbarIcon)
        }
    }

    /// The student's classes, which live in Account; pushed once.
    private func editClasses() {
        guard routes.last == .schedule else { return }
        routes.append(.manage)
    }

    private func goToToday() {
        withAnimation(reduceMotion ? nil : .snappy(duration: 0.22)) {
            state.selectedDate = state.today
            vertical.scrollTo(y: CGFloat(max(0, state.clock.minute - 60)) / 60 * hourHeight)
        }
    }

}

// MARK: - Week strip

/// The seven days of the selected week, the way Calendar heads its timeline.
/// The chosen day is a brand pill around its initial and number; today, when
/// it is not chosen, keeps its number in the brand color. On a phone the strip
/// sits in its own card under the day heading. In the week view it heads the
/// day columns instead, so it takes the hour gutter's width as a leading inset
/// and draws no card.
struct WeekStrip: View {
    @Environment(AppState.self) private var state
    @Binding var selected: CivilDate
    let today: CivilDate
    var leadingInset: CGFloat = 0
    var card = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @ScaledMetric(relativeTo: .title3) private var cell = 36.0

    /// The content card's corner, matching Home's cards.
    static let cardRadius: CGFloat = 24

    var body: some View {
        HStack(spacing: 0) {
            // A Spacer, not a clear color: a color would stretch to fill the
            // planner's height and push the timeline down.
            if leadingInset > 0 { Spacer(minLength: 0).frame(width: leadingInset) }
            ForEach(0..<7, id: \.self) { offset in
                let date = selected.startOfWeek().adding(days: offset)
                let chosen = date == selected
                let isToday = date == today
                Button {
                    withAnimation(reduceMotion ? nil : .snappy(duration: 0.22)) { selected = date }
                } label: {
                    VStack(spacing: 4) {
                        Text(Weekdays.initials[(date.weekdayIndex + 1) % 7])
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(isToday ? Color.brandInk : Color.inkMuted)
                        Text("\(date.dayOfMonth)")
                            .font(.display(17, relativeTo: .title3))
                            .monospacedDigit()
                            .lineLimit(1)
                            .minimumScaleFactor(0.7)
                            .foregroundStyle(chosen ? Color.onBrand : isToday ? Color.brandInk : date < today ? Color.inkMuted : Color.ink)
                            .frame(width: min(cell, 44), height: min(cell, 44))
                            .background {
                                if chosen { Circle().fill(isToday ? Color.brand : Color.ink) }
                            }
                        if card {
                            HStack(spacing: 3) {
                                ForEach(Array(ClassItem.list(state, on: date).prefix(4).enumerated()), id: \.offset) { _, item in
                                    Circle().fill(CoursePalette.tone(item.colorHex)).frame(width: 5, height: 5)
                                }
                            }
                            .frame(height: 5)
                            .opacity(date < today ? 0.5 : 1)
                        }
                    }
                    .padding(.vertical, 6)
                    .frame(maxWidth: .infinity)
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(date.longDay)
                .accessibilityValue(isToday ? "Today" : "")
                .accessibilityAddTraits(chosen ? .isSelected : [])
            }
        }
        .padding(.horizontal, card ? 4 : 0)
        .padding(.vertical, card ? 4 : 2)
        .background {
            if card {
                RoundedRectangle(cornerRadius: Self.cardRadius, style: .continuous).fill(Color.card)
                    .overlay {
                        RoundedRectangle(cornerRadius: Self.cardRadius, style: .continuous)
                            .strokeBorder(Color.hairline, lineWidth: 1)
                    }
            } else {
                Color.surface
            }
        }
    }
}

#if DEBUG
#Preview("Schedule") {
    @Previewable @State var routes: [ClassesRoute] = []
    NavigationStack {
        ScheduleView(routes: $routes).classesDestinations()
    }
    .environment(AppState.preview())
    .tint(.brand)
}
#endif
