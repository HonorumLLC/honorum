import SwiftUI

/// Honorum's main screen: one page, in the order students reach for it.
/// Today's tasks lead, then the rest of the week's, then the schedule, then
/// the term. The three progress rings mark the date at the top, and each
/// ring then heads the section it counts.
///
/// Every task, the full calendar, and Account are screens Home pushes. The
/// bottom bar searches tasks and adds a task or an event.
struct HomeView: View {
    @Environment(AppState.self) private var state
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var day: CivilDate?

    let open: (ClassesRoute) -> Void
    let edit: (StudentTask) -> Void
    let compose: () -> Void

    var body: some View {
        // The minute clock moves the now line, the rings at midnight, and
        // the day the page is about.
        TimelineView(.everyMinute) { context in
            page(HomeModel(state: state, at: instant(context.date)))
        }
    }

    /// The moment Home is about. In debug builds, `-homeClock 10:42` holds
    /// the campus time of day still for screenshots.
    private func instant(_ date: Date) -> Date {
        #if DEBUG
        if let text = UserDefaults.standard.string(forKey: "homeClock") {
            let parts = text.split(separator: ":").compactMap { Int($0) }
            if parts.count == 2 {
                return CivilDate(date, in: state.timeZone).instant(minute: parts[0] * 60 + parts[1], in: state.timeZone)
            }
        }
        #endif
        return date
    }

    private func page(_ model: HomeModel) -> some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    today(model)
                    week(model)
                    schedule(model)
                    term(model)
                    classes(model)
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 24)
                .frame(maxWidth: 640)
                .frame(maxWidth: .infinity)
            }
            .scrollIndicators(.hidden)
            .background(Color.surface.ignoresSafeArea())
            .task {
                #if DEBUG
                // `-homeScroll schedule` opens the page at a section for screenshots.
                if let anchor = UserDefaults.standard.string(forKey: "homeScroll") {
                    try? await Task.sleep(for: .milliseconds(300))
                    proxy.scrollTo(anchor, anchor: .top)
                }
                #endif
            }
        }
        .modifier(HomeSearch(compose: compose, edit: edit))
        // The title names the back button; the bar shows the dial and date.
        .navigationTitle(model.today.weekdayName)
        .toolbarTitleDisplayMode(.inline)
        .toolbar(removing: .title)
        .toolbar { bar(model) }
        .refreshable { await state.refreshAssignments(force: true) }
    }

    // MARK: Sections

    /// The navigation bar: the dial beside the campus date, then the streak
    /// and the way into Account. Being the system's bar, it stays put while
    /// the page bounces under it, and pull to refresh shows its spinner.
    @ToolbarContentBuilder private func bar(_ model: HomeModel) -> some ToolbarContent {
        ToolbarItem(placement: .topBarLeading) {
            HStack(spacing: 10) {
                RingDial(counts: model.counts, size: 40, stroke: 3.5, gap: 1.25)
                VStack(alignment: .leading, spacing: -3) {
                    Text(model.today.weekdayName)
                        .font(.display(24, relativeTo: .title))
                        .foregroundStyle(Color.ink)
                    Text(model.today.monthDay)
                        .font(.display(15, relativeTo: .subheadline))
                        .foregroundStyle(Color.inkMuted)
                }
                .lineLimit(1)
                .accessibilityElement(children: .combine)
                .accessibilityAddTraits(.isHeader)
            }
            .fixedSize()
        }
        .bareInBar()
        // One group, so the streak and Account share a glass capsule. The
        // streak opens Account too, where its page lives.
        ToolbarItemGroup(placement: .topBarTrailing) {
            if state.isSignedIn, let streak = state.streak?.current {
                Button { open(.account) } label: { StreakLabel(days: streak) }
            }
            Button("Account", systemImage: "person.fill") { open(.account) }
                .tint(Color.brandInk)
                .accessibilityIdentifier("accountButton")
        }
    }

    private func today(_ model: HomeModel) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            RingTitle(period: .today, title: "Today", count: model.counts[.today])
            DeadlineCard(items: model.todayTasks, empty: "Nothing due today", onToggle: toggle, onOpen: { edit($0.task) })
        }
        .padding(.top, 8)
        .id("tasks")
    }

    /// The rest of the week's work, after today's.
    private func week(_ model: HomeModel) -> some View {
        let last = model.week.last?.date ?? model.today
        let rest = model.upcoming.filter { ($0.date ?? last) <= last }
        return VStack(alignment: .leading, spacing: 10) {
            RingTitle(period: .week, title: "This week", count: model.counts[.week])
            TaskListCard(items: rest, today: model.today, empty: "Nothing else this week",
                         onToggle: toggle, onOpen: { edit($0.task) }, onAll: { open(.tasks) })
        }
        .padding(.top, 26)
    }

    private func schedule(_ model: HomeModel) -> some View {
        let chosen = day ?? model.today
        let isToday = chosen == model.today
        return VStack(alignment: .leading, spacing: 10) {
            SectionTitle(title: "Schedule", action: ("Calendar", "openSchedule", { showSchedule(chosen) }))
            DayStrip(days: model.week.map(\.date), today: model.today,
                     selected: Binding(get: { chosen }, set: { day = $0 }),
                     colors: { ClassItem.list(state, on: $0).map(\.colorHex) })
            DayAgenda(classes: isToday ? model.classes : ClassItem.list(state, on: chosen),
                      now: isToday ? model.now : nil,
                      empty: isToday ? "No classes today" : "No classes") { showSchedule(chosen) }
                .padding(.leading, -6)
                .padding(.top, 4)
                .id(chosen)
        }
        .padding(.top, 30)
        .id("schedule")
    }

    @ViewBuilder private func term(_ model: HomeModel) -> some View {
        if let term = model.term {
            VStack(alignment: .leading, spacing: 10) {
                RingTitle(period: .term, title: term.name, count: model.counts[.term])
                TermCard(term: term, milestones: model.milestones)
            }
            .padding(.top, 30)
            .id("progress")
        }
    }

    /// Each class's share of the term. Adding and removing classes lives in
    /// Account; an empty term points there.
    private func classes(_ model: HomeModel) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionTitle(title: "Classes")
            if model.courses.isEmpty {
                VStack(alignment: .leading, spacing: 12) {
                    Text("Add your classes to see them here and on the calendar.")
                        .font(.body)
                        .foregroundStyle(Color.inkSecondary)
                    Button("Add Classes") { open(.manage) }
                        .buttonStyle(.softPill)
                        .accessibilityIdentifier("manageClasses")
                }
                .card()
            } else {
                ClassTiles(courses: model.courses)
            }
        }
        .padding(.top, 30)
    }

    // MARK: Actions

    private func toggle(_ item: TaskItem) {
        withAnimation(reduceMotion ? nil : .smooth(duration: 0.35)) { state.toggle(taskID: item.id) }
        if !item.done { UIImpactFeedbackGenerator(style: .light).impactOccurred() }
    }

    private func showSchedule(_ date: CivilDate) {
        state.selectedDate = date
        open(.schedule)
    }
}

private extension ToolbarContent {
    /// Drawn as is, without the glass a bar item otherwise sits on.
    @ToolbarContentBuilder func bareInBar() -> some ToolbarContent {
        if #available(iOS 26.0, *) {
            sharedBackgroundVisibility(.hidden)
        } else {
            self
        }
    }
}

/// The account's streak, as a flame and a count, for a bar button: a plain
/// view rather than a Label, which a bar draws as its icon alone.
private struct StreakLabel: View {
    let days: Int

    var body: some View {
        HStack(spacing: 3) {
            StreakFlame(size: 17)
            Text("\(days)")
                .font(.body.weight(.semibold))
                .monospacedDigit()
                .foregroundStyle(Color.ink)
                .contentTransition(.numericText(value: Double(days)))
        }
        .fixedSize()
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Streak, \(days) \(days == 1 ? "day" : "days")")
        .accessibilityIdentifier("streak")
    }
}

#if DEBUG
#Preview("Home") {
    NavigationStack {
        HomeView(open: { _ in }, edit: { _ in }, compose: {})
    }
    .environment(AppState.preview())
}
#endif
