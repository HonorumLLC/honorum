import SwiftUI

/// Which day the schedule's chrome shows, kept by the day pager. There are two
/// answers, and each piece of chrome reads exactly one of them:
///
/// - `displayedDay` is the day filling most of the viewport. The day heading
///   and week strip show it, so they change together, once, as a page crosses
///   its midpoint.
/// - `restingDay` is the day the pager last rested on, which the selection
///   commits to.
///
/// A jump to a chosen day (the week strip, Today) moves both to the
/// destination at once and ignores the frames of the travel, so the chrome
/// changes once instead of flicking through every day the pager passes.
@MainActor
@Observable
final class PagerProgress {
    /// Nil until the pager first reports; the chrome keeps the selection.
    private(set) var displayedDay: Int?
    private(set) var restingDay: Int?
    /// True from a jump until the pager rests or a finger takes over.
    @ObservationIgnored private var traveling = false

    /// A frame of a swipe or its deceleration, in fractional days.
    func follow(page: Double) {
        guard !traveling, page.isFinite else { return }
        show(Int(page.rounded()))
    }

    /// The pager is leaving for `day` on its own.
    func jump(to day: Int) {
        traveling = true
        show(day, resting: true)
    }

    /// The pager has come to rest on `day`.
    func settle(on day: Int) {
        traveling = false
        show(day, resting: true)
    }

    /// A finger took hold mid-travel; the chrome follows it again.
    func interrupt() {
        traveling = false
    }

    /// Observers are only told about a day that actually changed.
    private func show(_ day: Int, resting: Bool = false) {
        if displayedDay != day { displayedDay = day }
        if resting, restingDay != day { restingDay = day }
    }

    /// The one fade the chrome uses whenever the day it shows changes.
    static func chromeAnimation(reduceMotion: Bool) -> Animation {
        .easeInOut(duration: reduceMotion ? 0.12 : 0.2)
    }
}

/// Stable, separate pages inside the shared vertical timeline. The system owns
/// dragging, velocity, cancellation, and settling; none of them rebuild the
/// schedule's occurrence layout on every movement of the finger.
struct DayPager: View {
    @Binding var selected: CivilDate
    let progress: PagerProgress
    let hourHeight: CGFloat
    let conflicts: Set<String>
    var onOpen: (Occurrence) -> Void

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.scenePhase) private var scenePhase
    @State private var days: ClosedRange<Int>
    @State private var scrolledDay: Int?
    @State private var phase: ScrollPhase = .idle
    /// The last offset the scroll view reported, in pages from the leading
    /// edge, so a return to the tab can tell whether it rests between days.
    @State private var lastPage: Double?
    /// How often the pager has come on screen, for the UI tests.
    @State private var appearances = 0
    /// An animated travel has been asked for but the scroll view has not yet
    /// left the idle phase. The position change lands before the phase does,
    /// so committing on it would settle the chrome and then report the frames.
    @State private var travelPending = false

    init(selected: Binding<CivilDate>, progress: PagerProgress, hourHeight: CGFloat,
         conflicts: Set<String>, onOpen: @escaping (Occurrence) -> Void) {
        _selected = selected
        self.progress = progress
        self.hourHeight = hourHeight
        self.conflicts = conflicts
        self.onOpen = onOpen
        let day = selected.wrappedValue.day
        _days = State(initialValue: (day - 31)...(day + 31))
        _scrolledDay = State(initialValue: day)
    }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView(.horizontal) {
                LazyHStack(spacing: 0) {
                    ForEach(days, id: \.self) { day in
                        DayColumn(date: CivilDate(day: day), hourHeight: hourHeight,
                                  conflicts: conflicts, onOpen: onOpen)
                            .containerRelativeFrame(.horizontal)
                            .frame(height: hourHeight * 24)
                            .clipped()
                            .transition(.identity)
                            .accessibilityElement(children: .contain)
                            .accessibilityLabel(CivilDate(day: day).longDay)
                            .accessibilityIdentifier("timeline-day-\(CivilDate(day: day).iso)")
                            .accessibilityHidden(day != selected.day)
                            .transaction { $0.animation = nil }
                    }
                }
                .scrollTargetLayout()
            }
            .scrollTargetBehavior(.viewAligned(limitBehavior: .alwaysByOne))
            .scrollPosition(id: $scrolledDay, anchor: .center)
            .scrollIndicators(.hidden)
            .accessibilityIdentifier("day-pages")
            .accessibilityValue(debugState)
            .onScrollGeometryChange(for: Double.self) { geometry in
                geometry.contentOffset.x / max(geometry.containerSize.width, 1)
            } action: { _, page in
                lastPage = page
                report(page: page)
            }
            .onScrollPhaseChange { _, newPhase in
                phase = newPhase
                if newPhase != .idle { travelPending = false }
                switch newPhase {
                case .idle:
                    commitSelection()
                    // A touch cancelled by a tab switch can end between days.
                    realign(with: proxy)
                case .interacting: progress.interrupt()
                default: break
                }
            }
            // Leaving the tab or the app mid-swipe stops the scroll view
            // wherever it was, between two days. Coming back squares it up.
            .onAppear {
                appearances += 1
                realign(with: proxy)
            }
            .onChange(of: scenePhase) { _, phase in
                if phase == .active { realign(with: proxy) }
            }
        }
        .onChange(of: scrolledDay) {
            if phase == .idle, !travelPending { commitSelection() }
        }
        .onChange(of: selected) { _, date in
            guard scrolledDay != date.day else { return }
            let nearby = abs(date.day - (scrolledDay ?? date.day)) <= 7
            let travels = !reduceMotion && nearby
            travelPending = travels
            withAnimation(travels ? .smooth(duration: 0.25) : nil) {
                extendDays(around: date.day)
                scrolledDay = date.day
            }
            // The chrome crossfades straight to the destination, whether the
            // timeline travels there or jumps.
            withAnimation(PagerProgress.chromeAnimation(reduceMotion: reduceMotion)) {
                progress.jump(to: date.day)
            }
        }
        .onAppear { progress.settle(on: selected.day) }
        .frame(height: hourHeight * 24)
        .clipped()
    }

    /// Where the pager is and what it is doing, for the UI tests only.
    private var debugState: String {
        guard ProcessInfo.processInfo.arguments.contains("-uiTesting") else { return "" }
        return String(format: "page %.2f phase %@ selected %d appearances %d", lastPage ?? -1, String(describing: phase), selected.day - days.lowerBound, appearances)
    }

    /// Squares the pager up on the nearest day when it rests between two.
    /// The scroll view stops wherever it was when its tab or app goes away
    /// mid-swipe, and the phase it last reported cannot be trusted afterwards,
    /// so the offset alone decides: the day most on screen becomes the
    /// selection, the way a finished swipe would have made it.
    private func realign(with proxy: ScrollViewProxy) {
        guard let lastPage else { return }
        let nearest = Int(lastPage.rounded())
        guard abs(lastPage - Double(nearest)) > 0.02 else { return }
        let day = days.lowerBound + nearest
        phase = .idle
        travelPending = false
        withAnimation(reduceMotion ? nil : .smooth(duration: 0.2)) {
            proxy.scrollTo(day, anchor: .center)
        }
        if scrolledDay != day { scrolledDay = day }
        commitSelection()
    }

    /// Pages are one container wide from the leading edge of `days`, so the
    /// offset in pages is the fractional day. While the pager rests, geometry
    /// only changes because pages were added or the selection jumped, and
    /// neither moves the day on screen.
    private func report(page: Double) {
        guard phase != .idle else { return }
        progress.follow(page: Double(days.lowerBound) + page)
    }

    /// Commit the settled day. The chrome changed as the page crossed its
    /// midpoint; what waits for rest is the selection.
    private func commitSelection() {
        guard let day = scrolledDay else { return }
        travelPending = false
        progress.settle(on: day)
        if selected.day != day {
            withAnimation(PagerProgress.chromeAnimation(reduceMotion: reduceMotion)) {
                selected = CivilDate(day: day)
            }
        }
        extendDays(around: day)
    }

    /// Grow near an edge without recycling or removing existing page identities.
    /// LazyHStack only builds the pages near the viewport, even across long terms.
    private func extendDays(around day: Int) {
        let lower = day < days.lowerBound + 7 ? day - 31 : days.lowerBound
        let upper = day > days.upperBound - 7 ? day + 31 : days.upperBound
        if lower != days.lowerBound || upper != days.upperBound {
            days = lower...upper
        }
    }
}
