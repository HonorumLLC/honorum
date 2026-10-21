#if DEBUG
import SwiftUI

/// `-screen name` opens one screen or sheet directly, over Home, so every
/// page can be captured the same way for design reviews.
enum DebugScreen {
    static var name: String { UserDefaults.standard.string(forKey: "screen") ?? "" }

    static var routes: [ClassesRoute]? {
        let course = UITestFixture.catalog.courses[0]
        return switch name {
        case "tasks": [.tasks]
        case "schedule": [.schedule]
        case "account": [.account]
        case "manage": [.account, .manage]
        case "classSearch": [.account, .manage, .search]
        case "course": [.account, .manage, .search, .course(course)]
        case "school": [.account, .manage, .school]
        case "term": [.account, .manage, .term]
        case "sources": [.account, .sources]
        case "alerts": [.account, .alerts]
        case "devices": [.account, .devices]
        case "copies": [.account, .copies]
        case "versions": [.account, .versions]
        default: nil
        }
    }

    /// The sheets, which belong to the screens that present them.
    static let sheets: Set<String> = ["newTask", "editTask", "newEvent", "editEvent", "occurrence", "schedules", "share", "signIn"]
}

struct DebugSheet: Identifiable {
    let id: String
}

/// The sheets the root does not present itself.
struct DebugSheetContent: View {
    @Environment(AppState.self) private var state
    let sheet: DebugSheet

    var body: some View {
        switch sheet.id {
        case "occurrence":
            if let occurrence = state.occurrences(on: state.today).first(where: { if case .section = $0.kind { true } else { false } }) {
                OccurrenceDetail(occurrence: occurrence, date: state.today) { _ in }
            }
        case "schedules": SchedulesView()
        case "share": ShareScheduleView()
        case "signIn": SignInView()
        default: EmptyView()
        }
    }
}
#endif
