import BackgroundTasks
import SwiftUI
import UIKit
import UserNotifications

/// Receives the APNs device token and notification taps; SwiftUI has no hook
/// for either.
@MainActor
final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    static var state: AppState?
    /// A notification tapped before the state existed, followed once it does.
    static var pendingNotification: String?

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        return true
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .sound]
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse
    ) async {
        let identifier = response.notification.request.identifier
        await MainActor.run {
            if let state = Self.state {
                state.route(notification: identifier)
            } else {
                Self.pendingNotification = identifier
            }
        }
    }

    func application(
        _ application: UIApplication,
        didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data
    ) {
        Self.state?.registerPush(token: deviceToken.map { String(format: "%02x", $0) }.joined())
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        // Local reminders keep working; the next launch tries again.
    }
}

@main
struct HonorumApp: App {
    /// The identifier iOS knows this app's opportunistic refresh by. It has to
    /// match `BGTaskSchedulerPermittedIdentifiers` in Info.plist.
    static let refreshTask = "app.honorum.ios.refresh"

    @UIApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var state = AppState()
    @Environment(\.scenePhase) private var scenePhase

    init() {
        Typography.install()
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(state)
                .tint(.brand)
                .task {
                    AppDelegate.state = state
                    if let identifier = AppDelegate.pendingNotification {
                        AppDelegate.pendingNotification = nil
                        state.route(notification: identifier)
                    }
                    await state.start()
                }
                .task(id: scenePhase) {
                    guard scenePhase == .active else { return }
                    // Coming forward is when the app catches up: the account, the
                    // watched seats, and any connected assignment source. The
                    // minute after that repeats everything but the seat check.
                    var cameForward = true
                    while !Task.isCancelled {
                        await state.recordForegroundActivity()
                        await state.syncNow()
                        state.startLiveUpdates()
                        if cameForward { await state.checkWatchedSeats() }
                        cameForward = false
                        await state.refreshAssignments()
                        do { try await Task.sleep(for: .seconds(60)) } catch { return }
                    }
                }
        }
        .onChange(of: scenePhase) { _, phase in
            guard phase != .active else { return }
            state.stopLiveUpdates()
            state.saveNow()
            Self.scheduleRefresh()
        }
        // iOS decides when a closed app may run. When it grants a turn, the
        // watched sections are what this app most wants to check.
        .backgroundTask(.appRefresh(Self.refreshTask)) {
            await MainActor.run { Self.scheduleRefresh() }
            await state.checkWatchedSeats()
            await state.syncNow()
            await Reminders.shared.rebuild(from: state)
        }
    }

    @MainActor
    private static func scheduleRefresh() {
        let request = BGAppRefreshTaskRequest(identifier: refreshTask)
        request.earliestBeginDate = Date(timeIntervalSinceNow: 15 * 60)
        try? BGTaskScheduler.shared.submit(request)
    }
}
