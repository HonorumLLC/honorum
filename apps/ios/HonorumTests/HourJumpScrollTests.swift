import CoreGraphics
import Testing

@testable import Honorum

@Suite("Flicking the timeline at least two hours, onto an hour")
struct HourJumpScrollTests {
    private let bottom: CGFloat = 52 * 24 - 520

    private func target(from origin: CGFloat, releasedAt release: CGFloat? = nil,
                        projected: CGFloat? = nil, velocity: CGFloat) -> CGFloat? {
        HourJumpScrollBehavior.jumpTarget(
            from: origin, releasedAt: release ?? origin, projected: projected, velocity: velocity,
            hourHeight: 52, maximum: bottom)
    }

    private func rule(_ hour: CGFloat) -> CGFloat { hour * 52 - HourGrid.labelClearance }

    @Test("A finger that stops before lifting leaves the timeline where it is")
    func placing() {
        #expect(target(from: 123, velocity: 0) == nil)
        #expect(target(from: 123, velocity: 0.05) == nil)
        #expect(target(from: 123, velocity: -0.05) == nil)
    }

    @Test("Even a gentle flick jumps two hours from where it began, onto an hour rule")
    func flick() {
        #expect(target(from: rule(7), releasedAt: rule(7) + 30, velocity: 0.2) == rule(9))
        #expect(target(from: rule(7), releasedAt: rule(7) + 30, velocity: 3) == rule(9))
        #expect(target(from: rule(12), releasedAt: rule(12) - 30, velocity: -0.2) == rule(10))
        // From between rules, the jump lands on the nearest one.
        #expect(target(from: rule(7) + 20, velocity: 0.5) == rule(9))
        #expect(target(from: rule(7) + 30, velocity: 0.5) == rule(10))
    }

    @Test("A stronger flick keeps its momentum to the hour nearest its natural stop")
    func momentum() {
        // Momentum that stops short of two hours still jumps two.
        #expect(target(from: rule(7), projected: rule(8), velocity: 0.4) == rule(9))
        // Momentum that runs further is not braked at two hours.
        #expect(target(from: rule(7), projected: rule(11) + 20, velocity: 2) == rule(11))
        #expect(target(from: rule(7), projected: rule(12) - 10, velocity: 2) == rule(12))
        #expect(target(from: rule(12), projected: rule(6) + 10, velocity: -2) == rule(6))
        #expect(target(from: rule(12), projected: rule(11), velocity: -0.4) == rule(10))
    }

    @Test("A flick during a jump steps on from where that jump was headed")
    func chained() {
        // The scroll view reports the interrupted jump's landing as the origin.
        #expect(target(from: rule(9), releasedAt: rule(8) + 10, velocity: 1) == rule(11))
    }

    @Test("A drag longer than a jump settles on the next rule ahead")
    func longDrag() {
        #expect(target(from: rule(7), releasedAt: rule(10) + 10, velocity: 0.3) == rule(11))
        #expect(target(from: rule(12), releasedAt: rule(8) - 10, velocity: -0.3) == rule(7))
    }

    @Test("A flick stops at either end of the day")
    func ends() {
        #expect(target(from: rule(1), velocity: -1) == 0)
        #expect(target(from: rule(14), velocity: 1) == bottom)
    }
}
