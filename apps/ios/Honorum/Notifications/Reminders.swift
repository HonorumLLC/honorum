import Foundation
import UserNotifications

/// Local reminders: fifteen minutes before a class meeting, an hour before a
/// task is due. iOS decides when a closed app may run, so the app renews the
/// next sixty notifications every time it is opened rather than promising an
/// indefinite rolling window.
@MainActor
final class Reminders {
    static let shared = Reminders()

    static let classLead = 15 * 60
    static let taskLead = 60 * 60
    static let maximumScheduled = 60
    private static let prefix = "honorum.reminder."

    private let center = UNUserNotificationCenter.current()
    private var rebuilding = false

    func requestPermission() async -> Bool {
        (try? await center.requestAuthorization(options: [.alert, .sound, .badge])) ?? false
    }

    func isAuthorized() async -> Bool {
        let settings = await center.notificationSettings()
        return settings.authorizationStatus == .authorized
            || settings.authorizationStatus == .provisional
    }

    /// Replaces every scheduled reminder with the next sixty from the current
    /// plan. Cancelled class days and completed tasks produce nothing.
    func rebuild(from state: AppState) async {
        guard !rebuilding else { return }
        rebuilding = true
        defer { rebuilding = false }

        let pending = await center.pendingNotificationRequests()
        center.removePendingNotificationRequests(
            withIdentifiers: pending.map(\.identifier).filter { $0.hasPrefix(Self.prefix) })

        guard state.library.reminders, await isAuthorized() else { return }
        // A registered phone receives the account's reminders from the server.
        if state.serverDeliversNotifications { return }

        for item in Self.upcoming(in: state) {
            let content = UNMutableNotificationContent()
            content.title = item.title
            content.body = item.body
            content.sound = .default
            let components = Calendar.current.dateComponents(
                [.year, .month, .day, .hour, .minute], from: item.fireAt)
            let request = UNNotificationRequest(
                identifier: Self.prefix + item.id,
                content: content,
                trigger: UNCalendarNotificationTrigger(dateMatching: components, repeats: false))
            try? await center.add(request)
        }
    }

    struct Item: Sendable {
        var id: String
        var title: String
        var body: String
        var fireAt: Date
    }

    /// The next reminders due, classes and tasks together, soonest first.
    static func upcoming(in state: AppState, from now: Date = Date()) -> [Item] {
        var items: [Item] = []
        let zone = state.timeZone
        let schedule = state.library.schedule
        let closures = state.closures
        let term = state.term

        // Four weeks is well past sixty reminders for any real schedule.
        var day = state.today
        let last = day.adding(days: 28)
        while day <= last {
            for block in Occurrences.onDay(day, schedule: schedule, term: term, closures: closures) {
                let starts = day.instant(minute: block.start, in: zone)
                let fireAt = starts.addingTimeInterval(-Double(classLead))
                guard fireAt > now else { continue }
                let where_ = block.location.isEmpty ? "" : " · \(block.location)"
                items.append(
                    Item(
                        id: "class.\(day.iso).\(block.id)",
                        title: block.title,
                        body: "Starts at \(Clock.time(block.start))\(where_)",
                        fireAt: fireAt))
            }
            day = day.adding(days: 1)
        }

        for task in state.library.tasks where !task.done {
            guard let due = task.dueDate else { continue }
            let fireAt = due.addingTimeInterval(-Double(taskLead))
            guard fireAt > now else { continue }
            let course = task.course.isEmpty ? "" : "\(task.course) · "
            items.append(
                Item(
                    id: "task.\(task.id)",
                    title: task.title,
                    body: "\(course)Due at \(due.formatted(date: .omitted, time: .shortened))",
                    fireAt: fireAt))
        }

        return items.sorted { $0.fireAt < $1.fireAt }.prefix(maximumScheduled).map { $0 }
    }

    /// Announces an open seat straight away.
    func announceSeat(_ watch: SeatWatch, available: Int) async {
        guard await isAuthorized() else { return }
        let content = UNMutableNotificationContent()
        content.title = "\(watch.label) has a seat"
        content.body = available == 1
            ? "1 seat is open in \(watch.title)."
            : "\(available) seats are open in \(watch.title)."
        content.sound = .default
        let request = UNNotificationRequest(
            identifier: "honorum.seat.\(watch.id).\(Int(Date().timeIntervalSince1970))",
            content: content, trigger: nil)
        try? await center.add(request)
    }
}
