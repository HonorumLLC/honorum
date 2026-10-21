import SwiftUI

extension View {
    /// Register custom planner controls with the system scroll-edge blur.
    @ViewBuilder
    func plannerTopBar<Bar: View>(@ViewBuilder content: () -> Bar) -> some View {
        if #available(iOS 26.0, *) {
            safeAreaBar(edge: .top, spacing: 0, content: content)
                .scrollEdgeEffectStyle(.soft, for: .top)
        } else {
            safeAreaInset(edge: .top, spacing: 0) {
                content().background(Color.surface)
            }
        }
    }
}

extension View {
    /// The Due row is chrome, like the day header above it: the same warm
    /// surface, with a hairline where the timeline begins beneath it.
    func dueBarSurface() -> some View { modifier(DueBarSurface()) }
}

private struct DueBarSurface: ViewModifier {
    @Environment(\.displayScale) private var displayScale

    func body(content: Content) -> some View {
        content
            .background(Color.surface)
            .overlay(alignment: .bottom) {
                Rectangle().fill(Color.hairline).frame(height: 1 / displayScale)
            }
    }
}

/// A native list with white cards on the warm surface.
/// Row content supplies its background. Section headers use `ListHeader`.
struct AppList<Content: View>: View {
    var surface = Color.surface
    @ViewBuilder var content: Content

    var body: some View {
        List {
            Group { content }
                .listRowBackground(Color.card)
                .listRowSeparatorTint(Color.hairline)
        }
        .scrollContentBackground(.hidden)
        .background(surface)
        .listSectionSpacing(22)
    }
}
