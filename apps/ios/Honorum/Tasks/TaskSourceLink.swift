import SwiftUI

/// Use the system's open result so an absent Canvas app falls back to the web.
/// Opening directly needs no installed-app query or credentials.
struct TaskSourceLink<Label: View>: View {
    @Environment(\.openURL) private var openURL
    let task: StudentTask
    @ViewBuilder var label: Label

    var body: some View {
        if task.sourceURL != nil {
            Button {
                task.openSource(using: openURL)
            } label: {
                label
            }
        }
    }
}

extension StudentTask {
    @MainActor
    func openSource(using openURL: OpenURLAction) {
        guard let webURL = sourceURL else { return }
        guard let appURL = canvasAppURL else {
            openURL(webURL)
            return
        }
        openURL(appURL) { accepted in
            if !accepted { openURL(webURL) }
        }
    }
}
