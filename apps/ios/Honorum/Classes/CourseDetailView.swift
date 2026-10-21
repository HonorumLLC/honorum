import SwiftUI

/// A course and its sections, with seat counts refreshed while it is open.
/// The search hands over the course it already has, so the sections are on
/// screen the moment the push lands rather than after a catalog round trip.
struct CourseDetailView: View {
    @Environment(AppState.self) private var state
    let course: Course

    @State private var seats: [String: Seats] = [:]
    @State private var notificationsOff = false

    var body: some View {
        AppList {
            Section {
                VStack(alignment: .leading, spacing: 8) {
                    Text(course.code)
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(Color.brandInk)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                        .background(Color.tintFill, in: .capsule)
                    Text(course.title)
                        .font(.display(24, relativeTo: .title2))
                        .foregroundStyle(Color.ink)
                        .fixedSize(horizontal: false, vertical: true)
                    Text(facts)
                        .font(.subheadline)
                        .foregroundStyle(Color.inkSecondary)
                }
                .padding(.vertical, 8)
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets(top: 0, leading: 4, bottom: 0, trailing: 4))
            }
            ForEach(course.sections) { section in
                Section {
                    SectionRow(
                        section: section, course: course, seats: seats[section.id],
                        saved: state.isSaved(section.id),
                        watching: state.isWatching(section.id),
                        toggleSaved: {
                            if state.isSaved(section.id) {
                                state.remove(sectionID: section.id)
                            } else {
                                state.add(section, from: course, seats: seats[section.id])
                            }
                        },
                        toggleWatch: {
                            if state.isWatching(section.id) {
                                state.unwatch(sectionID: section.id)
                            } else {
                                Task {
                                    if await !state.watch(
                                        section: section, in: course, seats: seats[section.id])
                                    {
                                        notificationsOff = true
                                    }
                                }
                            }
                        })
                }
            }
        }
        .listSectionSpacing(14)
        .navigationTitle(course.code)
        .navigationBarTitleDisplayMode(.inline)
        .notificationsOffAlert(isPresented: $notificationsOff, purpose: "seat alerts")
        .task(id: course.code) {
            seats = await state.catalog.result(for: course.code)?.seats ?? [:]
            let live = await state.liveSeats(for: course.sections.map(\.id))
            seats.merge(live) { _, new in new }
        }
    }

    /// The subject, credits, and terms the course runs in.
    private var facts: String {
        var parts = [course.subject]
        if let credits = course.sections.first?.credits, credits > 0 {
            parts.append("\(credits.formatted()) credits")
        }
        if let offered = course.offered, !offered.isEmpty {
            parts.append("Offered \(offered.joined(separator: ", "))")
        }
        return parts.filter { !$0.isEmpty }.joined(separator: " · ")
    }
}

/// One section: when it meets, who teaches it, whether it has room, and the
/// two things a student does with it — add it, or watch for a seat.
struct SectionRow: View {
    let section: CourseSection
    let course: Course
    let seats: Seats?
    let saved: Bool
    let watching: Bool
    var toggleSaved: () -> Void
    var toggleWatch: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("Section \(section.num)")
                    .font(.display(17, relativeTo: .headline))
                    .foregroundStyle(Color.ink)
                if let type = section.type, !type.isEmpty {
                    Text(type)
                        .font(.caption2.weight(.semibold))
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Capsule().fill(Color.tintFill))
                        .foregroundStyle(Color.brandInk)
                }
                Spacer(minLength: 4)
                SeatBadge(seats: seats)
            }
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(section.days.enumerated()), id: \.offset) { _, meeting in
                    fact("clock", "\(Weekdays.describe(meeting.days))  \(Clock.range(meeting.start, meeting.end))")
                    if let location = meeting.location, !location.isEmpty {
                        fact("mappin.and.ellipse", location)
                    }
                }
                if section.days.isEmpty {
                    fact("clock", section.mode ?? "No meeting times")
                }
                if !section.teachers.isEmpty {
                    fact("person", section.teachers.joined(separator: ", "))
                }
            }
            HStack(spacing: 10) {
                Button(action: toggleSaved) {
                    Label(saved ? "In Schedule" : "Add", systemImage: saved ? "checkmark" : "plus")
                }
                .buttonStyle(PillButtonStyle(prominent: !saved))
                .accessibilityLabel(saved ? "Remove section \(section.num)" : "Add section \(section.num)")
                if seats?.isOpen == false || watching {
                    Button(action: toggleWatch) {
                        Label(watching ? "Watching" : "Watch Seat", systemImage: watching ? "bell.fill" : "bell")
                    }
                    .buttonStyle(.softPill)
                    .accessibilityLabel(watching ? "Watching for a Seat" : "Watch for a Seat")
                    .accessibilityAddTraits(watching ? .isSelected : [])
                }
            }
            .padding(.top, 2)
        }
        .padding(.vertical, 8)
    }

    private func fact(_ symbol: String, _ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: symbol)
                .font(.footnote)
                .foregroundStyle(Color.inkMuted)
                .frame(width: 18)
            Text(text)
                .font(.subheadline)
                .foregroundStyle(Color.inkSecondary)
        }
    }
}

/// Seat counts, or an honest silence when the server does not know them.
struct SeatBadge: View {
    let seats: Seats?

    var body: some View {
        if let seats, seats.isKnown {
            HStack(spacing: 4) {
                Circle()
                    .fill(seats.isOpen ? Color.success : Color.danger)
                    .frame(width: 6, height: 6)
                Text(label(seats))
                    .font(.caption.weight(.medium))
                if seats.waitlist > 0 {
                    Text("· \(seats.waitlist) waiting").font(.caption)
                }
            }
            .foregroundStyle(seats.isOpen ? Color.inkSecondary : Color.danger)
            .padding(.top, 1)
        }
    }

    private func label(_ seats: Seats) -> String {
        guard seats.isOpen else { return "Full" }
        return seats.capacity > 0
            ? "\(seats.available) of \(seats.capacity) open" : "\(seats.available) open"
    }
}

#if DEBUG
#Preview("Course") {
    NavigationStack { CourseDetailView(course: UITestFixture.catalog.courses[0]) }
        .environment(AppState.preview())
        .tint(.brand)
}
#endif
