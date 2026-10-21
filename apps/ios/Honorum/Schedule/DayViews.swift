import SwiftUI

// A day's classes as Home shows them: the week as a strip of days with a dot
// per class, and the chosen day's classes on a rail with the now line.

/// A class meeting, exam, or event, as a block on the day's rail.
struct ClassItem: Identifiable, Hashable {
    let id: String
    let title: String
    let place: String
    let start: Int
    let end: Int
    let colorHex: String

    /// A day's classes, exams, and events, in order of the clock.
    @MainActor static func list(_ state: AppState, on date: CivilDate) -> [ClassItem] {
        state.occurrences(on: date).compactMap { occurrence in
            switch occurrence.kind {
            case .task: return nil
            case .section:
                return ClassItem(
                    id: occurrence.id, title: occurrence.subtitle.isEmpty ? occurrence.title : occurrence.subtitle,
                    place: occurrence.location, start: occurrence.start, end: occurrence.end,
                    colorHex: occurrence.colorHex ?? CoursePalette.eventHex)
            case .exam, .event:
                return ClassItem(
                    id: occurrence.id, title: occurrence.title, place: occurrence.location,
                    start: occurrence.start, end: occurrence.end,
                    colorHex: occurrence.colorHex ?? CoursePalette.eventHex)
            }
        }
    }
}

/// The week's days, the way Calendar heads its timeline, with a dot in each
/// class's color under the days it meets.
struct DayStrip: View {
    let days: [CivilDate]
    let today: CivilDate
    @Binding var selected: CivilDate
    let colors: (CivilDate) -> [String]

    var body: some View {
        HStack(spacing: 0) {
            ForEach(days, id: \.day) { date in
                cell(date)
            }
        }
        .padding(.vertical, 4)
        .card(padding: 4)
    }

    private func cell(_ date: CivilDate) -> some View {
        let chosen = date == selected
        let isToday = date == today
        let dots = colors(date)
        return Button {
            withAnimation(.smooth(duration: 0.3)) { selected = date }
        } label: {
            VStack(spacing: 5) {
                Text(Weekdays.initials[(date.weekdayIndex + 1) % 7])
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(isToday ? Color.brandInk : Color.inkMuted)
                Text("\(date.dayOfMonth)")
                    .font(.display(17, relativeTo: .headline))
                    .monospacedDigit()
                    .foregroundStyle(chosen ? Color.onBrand : isToday ? Color.brandInk : date < today ? Color.inkMuted : Color.ink)
                    .frame(width: 36, height: 36)
                    .background {
                        if chosen { Circle().fill(isToday ? Color.brand : Color.ink) }
                    }
                HStack(spacing: 3) {
                    ForEach(Array(dots.prefix(4).enumerated()), id: \.offset) { _, hex in
                        Circle().fill(CoursePalette.tone(hex)).frame(width: 5, height: 5)
                    }
                }
                .frame(height: 5)
                .opacity(date < today ? 0.5 : 1)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 6)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(date.longDay)
        .accessibilityValue("\(dots.count) \(dots.count == 1 ? "class" : "classes")")
        .accessibilityAddTraits(chosen ? .isSelected : [])
    }
}

/// A day's classes, exams, and events in order of the clock, as their pastel
/// blocks on a rail. On today the now line runs between what has happened and
/// what is next, and what came before now folds into one row.
struct DayAgenda: View {
    let classes: [ClassItem]
    /// The campus minute on the day shown, or nil for any other day.
    let now: Int?
    var empty = "No classes today"
    let onOpen: () -> Void
    @State private var showsEarlier = false

    @ScaledMetric(relativeTo: .caption) private var gutter: CGFloat = 62
    private let rail: CGFloat = 26

    var body: some View {
        let earlier = now.map { now in classes.filter { $0.end <= now } } ?? []
        let current = now.map { now in classes.filter { $0.start <= now && $0.end > now } } ?? []
        let later = now.map { now in classes.filter { $0.start > now } } ?? classes
        VStack(spacing: 0) {
            if classes.isEmpty {
                Text(empty)
                    .font(.body)
                    .foregroundStyle(Color.inkMuted)
                    .frame(maxWidth: .infinity, minHeight: 52, alignment: .leading)
                    .padding(.horizontal, 16)
                    .card(padding: 0)
            } else {
                if !earlier.isEmpty {
                    earlierRow(earlier.count)
                    if showsEarlier {
                        ForEach(earlier) { item in block(item).opacity(0.55) }
                    }
                }
                ForEach(current) { item in block(item) }
                if let now, !later.isEmpty || !current.isEmpty { nowRow(now) }
                ForEach(later) { item in block(item) }
            }
        }
        .background(alignment: .leading) {
            if !classes.isEmpty {
                Rectangle()
                    .fill(Color.hairline)
                    .frame(width: 2)
                    .padding(.leading, gutter + 10 + rail / 2 - 1)
                    .padding(.vertical, 12)
            }
        }
    }

    private func line<Node: View, Content: View>(
        time: String, timeColor: Color = .inkMuted, top: CGFloat,
        @ViewBuilder node: () -> Node, @ViewBuilder content: () -> Content
    ) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Text(time)
                .font(.caption.weight(.medium))
                .monospacedDigit()
                .foregroundStyle(timeColor)
                .lineLimit(1)
                .minimumScaleFactor(0.8)
                .frame(width: gutter, alignment: .trailing)
                .padding(.top, top)
            node()
                .frame(width: rail)
                .padding(.top, top - 2)
            content()
        }
    }

    private func block(_ item: ClassItem) -> some View {
        let next = now.flatMap { now in classes.first { $0.start > now } }?.id == item.id
        return Button(action: onOpen) {
            line(time: Clock.time(item.start), top: 14) {
                Circle()
                    .fill(CoursePalette.tone(item.colorHex))
                    .overlay { Circle().strokeBorder(Color.surface, lineWidth: 3) }
                    .frame(width: 16, height: 16)
            } content: {
                ClassBlock(item: item, tag: tag(item, next: next))
                    .frame(minHeight: max(52, CGFloat(item.end - item.start) * 0.9), alignment: .top)
            }
            .padding(.vertical, 5)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityHint("Opens the schedule")
    }

    private func tag(_ item: ClassItem, next: Bool) -> ClassBlock.Tag? {
        guard let now else { return nil }
        if next { return .next(item.start - now < 60 ? "in \(item.start - now) min" : "Next") }
        if item.start <= now && item.end > now { return .now }
        return nil
    }

    private func nowRow(_ now: Int) -> some View {
        line(time: Clock.time(now), timeColor: .nowLine, top: 0) {
            Circle().fill(Color.nowLine).frame(width: 10, height: 10).padding(.top, 5)
        } content: {
            Capsule().fill(Color.nowLine).frame(height: 2).padding(.top, 7)
        }
        .padding(.vertical, 6)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Now, \(Clock.time(now))")
    }

    private func earlierRow(_ count: Int) -> some View {
        Button {
            withAnimation(.smooth(duration: 0.35)) { showsEarlier.toggle() }
        } label: {
            line(time: "Earlier", top: 1) {
                Circle()
                    .strokeBorder(Color.inkMuted.opacity(0.6), lineWidth: 2)
                    .background(Circle().fill(Color.surface))
                    .frame(width: 12, height: 12)
                    .padding(.top, 3)
            } content: {
                HStack {
                    Text(count == 1 ? "1 class" : "\(count) classes")
                        .font(.subheadline)
                        .foregroundStyle(Color.inkMuted)
                    Spacer()
                    Image(systemName: "chevron.down")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(Color.inkMuted)
                        .rotationEffect(.degrees(showsEarlier ? 180 : 0))
                }
            }
            .padding(.vertical, 8)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(showsEarlier ? "Hide earlier classes" : "Show \(count) earlier \(count == 1 ? "class" : "classes")")
    }
}

/// A class's pastel block: its title, where it is and until when, and a tag
/// for the one happening now or next.
struct ClassBlock: View {
    enum Tag { case now, next(String) }

    let item: ClassItem
    var tag: Tag?

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline) {
                Text(item.title)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Color.eventInk)
                    .multilineTextAlignment(.leading)
                Spacer(minLength: 4)
                switch tag {
                case .next(let label):
                    Text(label)
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(Color.onNowLine)
                        .padding(.horizontal, 7)
                        .padding(.vertical, 3)
                        .background(Color.nowLine, in: .capsule)
                case .now:
                    Text("Now")
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(Color.eventInk)
                        .padding(.horizontal, 7)
                        .padding(.vertical, 3)
                        .background(Color.white.opacity(0.55), in: .capsule)
                case nil:
                    EmptyView()
                }
            }
            Text(item.place.isEmpty ? "until \(Clock.time(item.end))" : "\(item.place) · until \(Clock.time(item.end))")
                .font(.footnote)
                .foregroundStyle(Color.eventInkSecondary)
                .multilineTextAlignment(.leading)
        }
        .padding(12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(CoursePalette.fill(item.colorHex), in: .rect(cornerRadius: 16, style: .continuous))
    }
}
