import SwiftUI

/// The streak's flame, drawn as the web app draws it: a warm gradient from a
/// red base to an amber tip, with a lighter core. `celebration` flares it once
/// each time it increases, the way the web flame flares as a count rolls over.
struct StreakFlame: View {
    var size: CGFloat
    var celebration = 0
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ZStack {
            FlameShape(part: .outer)
                .fill(LinearGradient(
                    stops: [.init(color: .flameBase, location: 0.15), .init(color: .flameTip, location: 1)],
                    startPoint: .bottom, endPoint: .top))
            FlameShape(part: .core)
                .fill(Color.flameCore.opacity(0.85))
        }
        .frame(width: size, height: size)
        .keyframeAnimator(initialValue: Flare(), trigger: reduceMotion ? 0 : celebration) { flame, flare in
            flame
                .scaleEffect(flare.scale, anchor: .bottom)
                .offset(y: flare.lift)
        } keyframes: { _ in
            KeyframeTrack(\.scale) {
                CubicKeyframe(1.16, duration: 0.28)
                CubicKeyframe(1, duration: 0.52)
            }
            KeyframeTrack(\.lift) {
                CubicKeyframe(-3, duration: 0.28)
                CubicKeyframe(0, duration: 0.52)
            }
        }
        .accessibilityHidden(true)
    }

    private struct Flare {
        var scale: CGFloat = 1
        var lift: CGFloat = 0
    }
}

/// The web flame's two paths, in its 24-point view box.
private struct FlameShape: Shape {
    enum Part { case outer, core }
    let part: Part

    func path(in rect: CGRect) -> Path {
        var path = Path()
        func p(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: x, y: y) }
        switch part {
        case .outer:
            path.move(to: p(12, 21))
            path.addRelativeArc(center: p(12, 14.5), radius: 6.5, startAngle: .degrees(90), delta: .degrees(-90))
            path.addCurve(to: p(14.1, 3.7), control1: p(18.5, 10.2), control2: p(15.2, 7.9))
            path.addCurve(to: p(10.8, 10.2), control1: p(11.9, 5.8), control2: p(10.8, 8.1))
            path.addCurve(to: p(9.2, 6.9), control1: p(9.7, 9.1), control2: p(9.2, 8))
            path.addCurve(to: p(5.5, 14.5), control1: p(7, 9.6), control2: p(5.5, 12.3))
            path.addRelativeArc(center: p(12, 14.5), radius: 6.5, startAngle: .degrees(180), delta: .degrees(-90))
        case .core:
            path.move(to: p(12, 19.4))
            path.addRelativeArc(center: p(12, 16.1), radius: 3.3, startAngle: .degrees(90), delta: .degrees(-90))
            path.addCurve(to: p(13, 10.7), control1: p(15.3, 13.9), control2: p(13.7, 12.8))
            path.addCurve(to: p(11.2, 14.1), control1: p(11.9, 11.7), control2: p(11.3, 12.9))
            path.addCurve(to: p(10.2, 12.5), control1: p(10.7, 13.7), control2: p(10.3, 13.1))
            path.addCurve(to: p(8.7, 16.1), control1: p(9.2, 13.5), control2: p(8.7, 14.9))
            path.addRelativeArc(center: p(12, 16.1), radius: 3.3, startAngle: .degrees(180), delta: .degrees(-90))
        }
        path.closeSubpath()
        return path.applying(CGAffineTransform(translationX: rect.minX, y: rect.minY)
            .scaledBy(x: rect.width / 24, y: rect.height / 24))
    }
}

#Preview {
    HStack(spacing: 24) {
        StreakFlame(size: 20)
        StreakFlame(size: 34)
        StreakFlame(size: 60)
    }
    .padding()
}
