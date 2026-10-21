import SwiftUI

extension Animation {
    /// The odometer settle the streak's numbers roll with.
    static func roll(_ duration: Double) -> Animation { .timingCurve(0.25, 1.12, 0.4, 1, duration: duration) }
}

/// The Streak page: the account's run of days that ended with nothing
/// overdue, the best run, and the last four weeks as dots in the calendar's
/// week order. A guest sees an offer to sign in; an account whose streak has
/// not arrived yet sees the weeks empty.
struct StreakPage: View {
    let streak: AccountStreak?
    let signedIn: Bool
    let canSignIn: Bool
    let now: Date
    let zone: TimeZone
    var signIn: () -> Void
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.dynamicTypeSize) private var typeSize
    @ScaledMetric(relativeTo: .title3) private var scaledSize: CGFloat = 112
    @ScaledMetric(relativeTo: .body) private var scaledDot = 18.0
    @ScaledMetric(relativeTo: .body) private var scaledGap = 7.0
    @State private var displayed = 0
    @State private var appeared = false
    /// Counts increases of the streak, each of which flares the flame.
    @State private var celebration = 0
    @ScaledMetric(relativeTo: .title) private var flameSize = 34.0

    private struct Cell: Identifiable {
        let date: CivilDate
        let state: String
        let today: Bool
        var id: Int { date.day }
    }

    private var dot: CGFloat { min(scaledDot, 24) }
    private var gap: CGFloat { min(scaledGap, 9) }

    /// Four weeks ending with the current week, in the planner's week order.
    private var cells: [Cell] {
        let today = streak.flatMap { CivilDate($0.today) } ?? CivilDate(now, in: zone)
        let first = today.startOfWeek().adding(days: -21)
        let states = Dictionary((streak?.days ?? []).map { ($0.day, $0.state) }, uniquingKeysWith: { _, last in last })
        return (0..<28).map { offset in
            let date = first.adding(days: offset)
            return Cell(date: date, state: date > today ? "future" : states[date.iso] ?? "none", today: date == today)
        }
    }

    var body: some View {
        let layout = typeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: 14))
            : AnyLayout(HStackLayout(spacing: 14))
        layout {
            summary
                .frame(maxWidth: .infinity, alignment: .leading)
            weeks
        }
        .frame(minHeight: min(scaledSize, 150))
        .onAppear {
            appeared = true
            update()
        }
        .onChange(of: streak?.current) { _, _ in update() }
    }

    private var flame: some View {
        StreakFlame(size: flameSize, celebration: celebration)
    }

    @ViewBuilder private var summary: some View {
        if let streak {
            VStack(alignment: .leading, spacing: 2) {
                HStack(alignment: .firstTextBaseline, spacing: 5) {
                    // The flame's base sits a little below the digits' baseline.
                    flame.alignmentGuide(.firstTextBaseline) { $0[.bottom] - $0.height * 3 / 24 - 3 }
                    Text(displayed, format: .number)
                        .font(.system(.largeTitle, design: .rounded, weight: .medium))
                        .monospacedDigit()
                        .contentTransition(.numericText(value: Double(displayed)))
                        .foregroundStyle(Color.ink)
                    Text(streak.current == 1 ? "day" : "days")
                        .font(.subheadline.weight(.medium))
                        .foregroundStyle(Color.inkSecondary)
                }
                Text("Best \(streak.longest)")
                    .font(.caption)
                    .monospacedDigit()
                    .foregroundStyle(Color.inkMuted)
                if let overdue = streak.overdue, overdue > 0 {
                    Text("\(overdue) overdue")
                        .font(.caption.weight(.semibold))
                        .monospacedDigit()
                        .foregroundStyle(Color.danger)
                }
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(
                "Streak: \(streak.current) \(streak.current == 1 ? "day" : "days"). Best \(streak.longest)."
                    + ((streak.overdue ?? 0) > 0 ? " \(streak.overdue ?? 0) overdue." : ""))
            .accessibilityIdentifier("streak-summary")
        } else if !signedIn {
            VStack(alignment: .leading, spacing: 6) {
                flame
                Text("Keep a Streak")
                    .font(.headline)
                    .foregroundStyle(Color.ink)
                if canSignIn {
                    Button("Sign In", action: signIn)
                        .buttonStyle(.borderedProminent)
                } else {
                    Text("Sign-in is unavailable.")
                        .font(.caption)
                        .foregroundStyle(Color.inkMuted)
                }
            }
        } else {
            VStack(alignment: .leading, spacing: 4) {
                flame
                Text("Streak")
                    .font(.headline)
                    .foregroundStyle(Color.ink)
                Text("Connect to update")
                    .font(.caption)
                    .foregroundStyle(Color.inkMuted)
            }
        }
    }

    private var weeks: some View {
        let cells = cells
        let kept = cells.filter { $0.state == "clean" }.count
        return Grid(horizontalSpacing: gap, verticalSpacing: gap) {
            ForEach(0..<4, id: \.self) { row in
                GridRow {
                    ForEach(0..<7, id: \.self) { column in
                        let index = row * 7 + column
                        let cell = cells[index]
                        Circle()
                            .fill(fill(for: cell.state))
                            .frame(width: dot, height: dot)
                            .overlay {
                                if cell.today {
                                    Circle()
                                        .strokeBorder(cell.state == "overdue" ? Color.danger : Color.brandInk, lineWidth: 1.75)
                                        .padding(-3.5)
                                }
                            }
                            .opacity(appeared || reduceMotion ? 1 : 0)
                            .scaleEffect(appeared || reduceMotion ? 1 : 0.6)
                            .animation(reduceMotion ? nil : .easeOut(duration: 0.5).delay(0.08 + Double(index) * 0.016), value: appeared)
                    }
                }
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(streak == nil ? "" : "\(kept) of the last 28 days ended with nothing overdue")
        .accessibilityHidden(streak == nil)
        .accessibilityIdentifier("streak-weeks")
    }

    private func fill(for state: String) -> Color {
        switch state {
        case "clean": Color.brand
        case "missed": Color.inkMuted.opacity(0.3)
        case "overdue": Color.danger.opacity(0.18)
        default: Color.hairline
        }
    }

    private func update() {
        let count = streak?.current ?? 0
        if displayed > 0, count > displayed { celebration += 1 }
        withAnimation(reduceMotion ? nil : .roll(1).delay(displayed == 0 ? 0.18 : 0.4)) {
            displayed = count
        }
    }
}
