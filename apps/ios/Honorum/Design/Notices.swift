import SwiftUI
import UIKit

extension View {
    /// Shows the app's pending notice as an alert, once, wherever it is attached.
    func userNoticeAlert() -> some View { modifier(UserNoticeAlert()) }

    /// Explains a switch that would not turn on because iOS has refused
    /// notifications, with the way to the app's notification settings.
    func notificationsOffAlert(isPresented: Binding<Bool>, purpose: String) -> some View {
        modifier(NotificationsOffAlert(isPresented: isPresented, purpose: purpose))
    }
}

private struct UserNoticeAlert: ViewModifier {
    @Environment(AppState.self) private var state

    func body(content: Content) -> some View {
        @Bindable var state = state
        content.alert(
            state.notice?.title ?? "",
            isPresented: Binding(get: { state.notice != nil }, set: { if !$0 { state.notice = nil } }),
            presenting: state.notice
        ) { _ in
            Button("OK") {}
        } message: { notice in
            Text(notice.message)
        }
    }
}

private struct NotificationsOffAlert: ViewModifier {
    @Binding var isPresented: Bool
    let purpose: String
    @Environment(\.openURL) private var openURL

    func body(content: Content) -> some View {
        content.alert("Notifications Are Off", isPresented: $isPresented) {
            Button("Open Settings") {
                if let url = URL(string: UIApplication.openNotificationSettingsURLString) { openURL(url) }
            }
            Button("Not Now", role: .cancel) {}
        } message: {
            Text("Allow notifications for Honorum in Settings to receive \(purpose).")
        }
    }
}
