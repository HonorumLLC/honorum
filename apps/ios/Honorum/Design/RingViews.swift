import SwiftUI

/// How much of a period's work is done.
struct TaskCount: Hashable, Sendable {
    var done: Int
    var total: Int

    /// A period with nothing in it is complete, as the rings count it.
    var fraction: Double { total == 0 ? 1 : Double(done) / Double(total) }
    var percent: Int { Int((fraction * 100).rounded()) }
    var left: Int { total - done }
}

/// One progress ring: a track in the ring's color mixed into the paper, and
/// an arc that sweeps in the way the web's do (a quart ease-out over 1.25 s).
/// A closed ring can show a check.
struct ProgressRing: View {
    let fraction: Double
    let color: Color
    var lineWidth: CGFloat = 4
    var delay: Double = 0
    var showsCheck = false

    @State private var drawn = 0.0
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ZStack {
            Circle().stroke(Color.surface.mix(with: color, by: 0.32), lineWidth: lineWidth)
            Circle()
                .trim(from: 0, to: drawn)
                .stroke(color, style: StrokeStyle(lineWidth: lineWidth, lineCap: .round))
                .rotationEffect(.degrees(-90))
                .opacity(drawn > 0.001 ? 1 : 0)
            if showsCheck && fraction >= 1 {
                Image(systemName: "checkmark")
                    .font(.system(size: lineWidth * 2.2, weight: .heavy))
                    .foregroundStyle(color)
            }
        }
        .padding(lineWidth / 2)
        .onAppear(perform: draw)
        .onChange(of: fraction) { _, _ in draw() }
        .accessibilityHidden(true)
    }

    private func draw() {
        guard !reduceMotion else { drawn = fraction; return }
        withAnimation(.timingCurve(0.25, 1, 0.5, 1, duration: 1.25).delay(delay)) { drawn = fraction }
    }
}

/// The three rings nested, today outermost, the way the web draws them.
struct RingDial: View {
    let counts: [RingPeriod: TaskCount]
    var size: CGFloat = 104
    var stroke: CGFloat = 10
    var gap: CGFloat = 2.5

    var body: some View {
        ZStack {
            ForEach(Array(RingPeriod.allCases.enumerated()), id: \.element) { index, period in
                let diameter = size - CGFloat(index) * 2 * (stroke + gap)
                ProgressRing(fraction: counts[period]?.fraction ?? 0, color: period.color,
                             lineWidth: stroke, delay: 0.12 * Double(index))
                    .frame(width: diameter, height: diameter)
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// A section title led by the ring that counts it.
struct RingTitle: View {
    let period: RingPeriod
    let title: String
    let count: TaskCount?

    var body: some View {
        let count = count ?? TaskCount(done: 0, total: 0)
        HStack(spacing: 10) {
            ProgressRing(fraction: count.fraction, color: period.color, lineWidth: 5.5, showsCheck: true)
                .frame(width: 30, height: 30)
            Text(title)
                .font(.display(19, relativeTo: .headline))
                .foregroundStyle(Color.ink)
            Spacer()
            Text("\(count.done) of \(count.total)")
                .font(.subheadline.weight(.semibold))
                .monospacedDigit()
                .foregroundStyle(period.ink)
        }
        .padding(.horizontal, 2)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isHeader)
        .accessibilityLabel("\(title), \(count.done) of \(count.total) done")
        .accessibilityIdentifier("ring-\(period.rawValue)")
    }
}
