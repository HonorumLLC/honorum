import SwiftUI
import UIKit

/// The term's schedules: which one is open, which one is primary, and the
/// ordinary ways to make more of them.
struct SchedulesView: View {
    @Environment(AppState.self) private var state
    @Environment(\.dismiss) private var dismiss

    @State private var naming = false
    @State private var newName = ""
    @State private var duplicating = false
    @State private var renamingID: String?
    @State private var deleting: Schedule?

    private var trimmedName: String { newName.trimmingCharacters(in: .whitespaces) }

    var body: some View {
        NavigationStack {
            AppList {
                Section {
                    ForEach(state.library.termSchedules) { plan in
                        Button {
                            state.open(scheduleID: plan.id)
                            dismiss()
                        } label: {
                            HStack(spacing: 12) {
                                swatches(plan)
                                VStack(alignment: .leading, spacing: 2) {
                                    HStack(spacing: 6) {
                                        Text(plan.name)
                                            .font(.display(16, relativeTo: .body))
                                            .foregroundStyle(Color.ink)
                                        if plan.primary {
                                            Image(systemName: "star.fill")
                                                .font(.caption)
                                                .foregroundStyle(Color.flame)
                                                .accessibilityLabel("Primary")
                                        }
                                    }
                                    Text(summary(plan))
                                        .font(.footnote)
                                        .foregroundStyle(Color.inkMuted)
                                }
                                Spacer()
                                if plan.id == state.library.current {
                                    Image(systemName: "checkmark").foregroundStyle(Color.brand)
                                        .accessibilityHidden(true)
                                }
                            }
                        }
                        .accessibilityAddTraits(plan.id == state.library.current ? .isSelected : [])
                        // No full swipe: a schedule is not deleted by accident.
                        .swipeActions(allowsFullSwipe: false) {
                            if state.library.termSchedules.count > 1 {
                                Button("Delete", systemImage: "trash", role: .destructive) {
                                    deleting = plan
                                }
                            }
                            Button("Rename", systemImage: "pencil") {
                                newName = plan.name
                                renamingID = plan.id
                            }
                            .tint(Color.brand)
                            if !plan.primary {
                                Button("Primary", systemImage: "star") {
                                    state.makePrimary(scheduleID: plan.id)
                                }
                                .tint(Color.flame)
                            }
                        }
                    }
                } header: {
                    ListHeader(state.term?.name ?? "This term")
                } footer: {
                    Text("Swipe a schedule to rename it, make it primary, or delete it.")
                }

                Section {
                    Button {
                        newName = "Schedule \(state.library.termSchedules.count + 1)"
                        duplicating = false
                        naming = true
                    } label: {
                        TileLabel("New Schedule", "plus", hex: Tiles.lavender)
                    }
                    Button {
                        newName = "\(state.schedule.name) copy"
                        duplicating = true
                        naming = true
                    } label: {
                        TileLabel("Duplicate This One", "plus.square.on.square", hex: Tiles.lavender)
                    }
                }
            }
            .navigationTitle("Schedules")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                        .tint(Color.toolbarIcon)
                }
            }
            .alert("New Schedule", isPresented: $naming) {
                TextField("Name", text: $newName)
                Button("Cancel", role: .cancel) {}
                Button("Create") {
                    state.newSchedule(
                        named: trimmedName, copying: duplicating ? state.schedule : nil)
                    dismiss()
                }
                .disabled(trimmedName.isEmpty)
            }
            .alert(
                "Rename Schedule",
                isPresented: Binding(get: { renamingID != nil }, set: { if !$0 { renamingID = nil } })
            ) {
                TextField("Name", text: $newName)
                Button("Cancel", role: .cancel) { renamingID = nil }
                Button("Save") {
                    if let id = renamingID { state.rename(scheduleID: id, to: trimmedName) }
                    renamingID = nil
                }
                .disabled(trimmedName.isEmpty)
            }
            .confirmationDialog(
                "Delete this schedule?",
                isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
                titleVisibility: .visible, presenting: deleting
            ) { plan in
                Button("Delete \(plan.name)", role: .destructive) { state.deleteSchedule(id: plan.id) }
            } message: { plan in
                Text("Its \(plan.sections.count) classes and \(plan.events.count) events go with it.")
            }
        }
    }

    /// The schedule's classes as overlapping dots in their colors.
    private func swatches(_ plan: Schedule) -> some View {
        let colors = plan.sections.prefix(4).map { plan.color(for: $0.course) }
        return ZStack {
            ForEach(Array(colors.enumerated()), id: \.offset) { index, hex in
                Circle()
                    .fill(CoursePalette.fill(hex))
                    .overlay { Circle().strokeBorder(Color.card, lineWidth: 2) }
                    .frame(width: 22, height: 22)
                    .offset(x: CGFloat(index) * 9 - CGFloat(colors.count - 1) * 4.5)
            }
            if colors.isEmpty {
                Circle().strokeBorder(Color.hairline, lineWidth: 1.5).frame(width: 22, height: 22)
            }
        }
        .frame(width: 52, height: 30)
        .accessibilityHidden(true)
    }

    private func summary(_ plan: Schedule) -> String {
        let credits = plan.credits
        let hours = credits == credits.rounded() ? String(Int(credits)) : String(format: "%.1f", credits)
        return "\(plan.sections.count) classes · \(hours) credits"
    }
}

// MARK: - Sharing

/// Exporting a schedule: an iCalendar file for any calendar app, or the plain
/// text of the week.
struct ShareScheduleView: View {
    @Environment(AppState.self) private var state
    @Environment(\.dismiss) private var dismiss
    @State private var exported: URL?

    var body: some View {
        NavigationStack {
            AppList {
                Section {
                    Button {
                        exported = write(ICalendarExport.file(for: state))
                    } label: {
                        TileLabel("Export to Calendar", "calendar.badge.plus", hex: Tiles.blue)
                    }
                    ShareLink(item: ICalendarExport.summary(for: state)) {
                        TileLabel("Share as Text", "text.alignleft", hex: Tiles.mint)
                    }
                } header: {
                    ListHeader(state.schedule.name)
                } footer: {
                    Text(
                        "The calendar file carries every meeting of the term, skipping campus closures and the days you took off."
                    )
                }
            }
            .navigationTitle("Share Schedule")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                        .tint(Color.toolbarIcon)
                }
            }
            .sheet(item: $exported) { url in
                ActivityView(items: [url])
            }
        }
        .presentationDetents([.medium])
    }

    private func write(_ text: String) -> URL? {
        let name = state.schedule.name.replacingOccurrences(of: "/", with: "-")
        let url = URL.temporaryDirectory.appending(path: "\(name).ics")
        do {
            try Data(text.utf8).write(to: url, options: .atomic)
        } catch {
            state.report("Could Not Share Schedule", error)
            return nil
        }
        return url
    }
}

extension URL: @retroactive Identifiable {
    public var id: String { absoluteString }
}

/// The system share sheet, for handing a file to another app.
struct ActivityView: UIViewControllerRepresentable {
    let items: [Any]

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: items, applicationActivities: nil)
    }

    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}
