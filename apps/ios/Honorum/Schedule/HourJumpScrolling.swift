import SwiftUI

/// Vertical timeline scrolling that moves at least two hours when flicked,
/// settles on an hour, and stays wherever a finger leaves it otherwise.
///
/// Any flick, even a gentle one, carries the timeline at least two hours from
/// where the gesture began and settles with an hour just under the chrome, the
/// way the timeline opens on the morning. A stronger flick keeps its momentum
/// and comes to rest on the hour nearest where it would have stopped anyway,
/// so the scroll view decelerates at its own rate instead of braking hard at
/// two hours. Flicking again before it lands steps on from that landing. A
/// finger that stops before lifting keeps the timeline where it is, so holding
/// and dragging still scrolls continuously.
///
/// This is the scroll view's own hook for where a gesture ends, the SwiftUI
/// counterpart of UIKit's `scrollViewWillEndDragging(_:withVelocity:
/// targetContentOffset:)` and the one `.paging` uses. The scroll view keeps
/// tracking the finger and animates to the landing itself, with no gesture of
/// ours competing for the touch.
struct HourJumpScrollBehavior: ScrollTargetBehavior {
    /// Lift-off speed, in points per millisecond, below which the finger was
    /// placing the timeline rather than flicking it. A finger that stops
    /// before lifting reads zero; the lightest flick reads a few tenths.
    static let flickVelocity: CGFloat = 0.1
    /// How far even the lightest flick moves the timeline.
    static let jumpHours: CGFloat = 2

    let hourHeight: CGFloat
    let position: TimelinePosition

    func updateTarget(_ target: inout ScrollTarget, context: TargetContext) {
        guard context.axes.contains(.vertical), let y = Self.jumpTarget(
            from: context.originalTarget.rect.minY, releasedAt: position.y,
            projected: target.rect.minY, velocity: context.velocity.dy, hourHeight: hourHeight,
            maximum: context.contentSize.height - context.containerSize.height)
        else { return }
        target.rect.origin.y = y
    }

    /// Where a flick should settle, or nil when the gesture was not a flick.
    /// `origin` is where the gesture began, or where an interrupted flick was
    /// headed; `release` is where the finger lifted; `projected` is where the
    /// scroll view's own deceleration would stop. Offsets are content
    /// positions at the top of the visible timeline, the frame
    /// `ScrollPosition` and scroll targets share.
    static func jumpTarget(
        from origin: CGFloat, releasedAt release: CGFloat, projected: CGFloat? = nil,
        velocity: CGFloat, hourHeight: CGFloat, maximum: CGFloat
    ) -> CGFloat? {
        guard abs(velocity) >= flickVelocity, hourHeight > 0, maximum > 0 else { return nil }
        func hour(_ offset: CGFloat) -> CGFloat { (offset + HourGrid.labelClearance) / hourHeight }
        var landing = (hour(origin) + (velocity > 0 ? jumpHours : -jumpHours)).rounded()
        // Momentum that would carry past the jump keeps going, to the hour
        // nearest its natural stop, so a stronger flick never brakes short.
        if let projected {
            let natural = hour(projected).rounded()
            landing = velocity > 0 ? max(landing, natural) : min(landing, natural)
        }
        // A drag longer than a jump has already passed that rule, so the
        // flick settles on the next one ahead rather than pulling back.
        landing = velocity > 0
            ? max(landing, hour(release).rounded(.up))
            : min(landing, hour(release).rounded(.down))
        return min(max(landing * hourHeight - HourGrid.labelClearance, 0), maximum)
    }
}

/// Where the timeline's scroll view is as it moves. A scroll target behavior
/// is told only where a gesture began and where momentum would carry it, so
/// the timeline records its offset here for a drag that outruns its jump. It
/// is a plain reference rather than state, since scrolling a frame should
/// never re-render the view.
final class TimelinePosition {
    var y: CGFloat = 0
}

private struct HourJumpScrolling: ViewModifier {
    let hourHeight: CGFloat
    @State private var position = TimelinePosition()

    func body(content: Content) -> some View {
        content
            .scrollTargetBehavior(HourJumpScrollBehavior(hourHeight: hourHeight, position: position))
            .onScrollGeometryChange(for: CGFloat.self) { geometry in
                geometry.contentOffset.y + geometry.contentInsets.top
            } action: { _, y in
                position.y = y
            }
    }
}

extension View {
    /// Flicks move the timeline at least two hours, onto an hour; held drags scroll freely.
    func hourJumpScrolling(hourHeight: CGFloat) -> some View {
        modifier(HourJumpScrolling(hourHeight: hourHeight))
    }
}
