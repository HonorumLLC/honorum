import SwiftUI

/// What a tapped block on the planner is: a class meeting or the student's own
/// event, with the actions that belong to it.
struct OccurrenceDetail: View {
    @Environment(AppState.self) private var state
    @Environment(\.dismiss) private var dismiss

    let occurrence: Occurrence
    let date: CivilDate
    var edit: (PersonalEvent) -> Void

    @State private var confirmingRemoval = false

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 12) {
                    hero
                    switch occurrence.kind {
                    case .task: EmptyView()
                    case .section(let id), .exam(let id, _):
                        if let saved = state.section(id: id) { sectionDetails(saved) }
                    case .event(let id):
                        if let event = state.event(id: id) { eventActions(event) }
                    }
                    VStack(spacing: 8) {
                        Button(isExam ? "Hide This Exam" : "Skip This Day", systemImage: "calendar.badge.minus") {
                            state.skip(itemID: occurrence.itemID, on: date)
                            dismiss()
                        }
                        .buttonStyle(.softPill)
                        Text(
                            isExam
                                ? "Takes this sitting off the calendar; the class stays as it is."
                                : "Takes this one meeting off the week and leaves the rest in place.")
                            .font(.footnote)
                            .foregroundStyle(Color.inkMuted)
                            .multilineTextAlignment(.center)
                    }
                    .padding(.top, 4)
                    if case .section(let id) = occurrence.kind, let saved = state.section(id: id) {
                        Button("Remove from Schedule", role: .destructive) { confirmingRemoval = true }
                            .buttonStyle(.softPill)
                            .foregroundStyle(Color.danger)
                            .confirmationDialog(
                                "Remove \(saved.label)?", isPresented: $confirmingRemoval, titleVisibility: .visible
                            ) {
                                Button("Remove", role: .destructive) {
                                    state.remove(sectionID: saved.id)
                                    dismiss()
                                }
                            }
                    }
                }
                .padding(16)
            }
            .background(Color.surface.ignoresSafeArea())
            .navigationTitle(occurrence.title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                        .tint(Color.toolbarIcon)
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }

    /// The block itself, larger: what it is, its name, when and where.
    private var hero: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(kindLabel.uppercased())
                .font(.caption2.weight(.bold))
                .tracking(0.6)
                .foregroundStyle(Color.eventInkSecondary)
            Text(heading)
                .font(.display(24, relativeTo: .title2))
                .foregroundStyle(Color.eventInk)
                .fixedSize(horizontal: false, vertical: true)
            VStack(alignment: .leading, spacing: 4) {
                Label("\(date.mediumDay) · \(Clock.range(occurrence.start, occurrence.end))", systemImage: "clock")
                if !occurrence.location.isEmpty {
                    Label(occurrence.location, systemImage: "mappin.and.ellipse")
                }
            }
            .font(.subheadline)
            .foregroundStyle(Color.eventInkSecondary)
            .padding(.top, 4)
        }
        .padding(20)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(CoursePalette.fill(occurrence.colorHex), in: .rect(cornerRadius: 24, style: .continuous))
        .accessibilityElement(children: .combine)
    }

    private var heading: String {
        if case .section = occurrence.kind, !occurrence.subtitle.isEmpty { return occurrence.subtitle }
        return occurrence.title
    }

    private var kindLabel: String {
        switch occurrence.kind {
        case .section: occurrence.subtitle.isEmpty ? "Class" : occurrence.title
        case .exam: "Exam"
        case .event: "Event"
        case .task: "Task"
        }
    }

    private var isExam: Bool {
        if case .exam = occurrence.kind { return true }
        return false
    }

    @ViewBuilder private func sectionDetails(_ saved: SavedSection) -> some View {
        EditorCard(label: "Section \(saved.num)") {
            VStack(spacing: 0) {
                if !saved.instructors.isEmpty {
                    fact("Instructor", saved.instructors.joined(separator: ", "))
                }
                ForEach(Array(saved.meetings.enumerated()), id: \.offset) { _, meeting in
                    fact(Weekdays.describe(meeting.days), Clock.range(meeting.start, meeting.end))
                }
                ForEach(Array(saved.exams.enumerated()), id: \.offset) { _, exam in
                    fact("Exam", "\(CivilDate(exam.date)?.mediumDay ?? exam.date), \(Clock.time(exam.start))")
                }
                if saved.credits > 0 { fact("Credits", saved.credits.formatted()) }
                if !saved.mode.isEmpty { fact("Mode", saved.mode) }
                if !saved.type.isEmpty { fact("Type", saved.type) }
            }
            SeatBadge(seats: state.seatCounts[saved.id] ?? saved.seats)
        }
    }

    private func fact(_ label: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(label).foregroundStyle(Color.inkSecondary)
            Spacer(minLength: 12)
            Text(value).foregroundStyle(Color.ink).multilineTextAlignment(.trailing)
        }
        .font(.subheadline)
        .frame(minHeight: 34)
    }

    @ViewBuilder private func eventActions(_ event: PersonalEvent) -> some View {
        if !event.notes.isEmpty {
            EditorCard(label: "Notes") {
                Text(event.notes).foregroundStyle(Color.ink)
            }
        }
        HStack(spacing: 10) {
            Button("Edit", systemImage: "pencil") {
                edit(event)
                dismiss()
            }
            .buttonStyle(.pill)
            Button("Delete", systemImage: "trash", role: .destructive) {
                state.removeEvent(id: event.id)
                dismiss()
            }
            .buttonStyle(.softPill)
            .foregroundStyle(Color.danger)
        }
    }
}
