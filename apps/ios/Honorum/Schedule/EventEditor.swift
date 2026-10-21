import SwiftUI

/// Adds or edits one of the student's own events, in the same cards as the
/// task editor: its name, when it happens, its class or color, and notes.
struct EventEditor: View {
    @Environment(AppState.self) private var state
    @Environment(\.dismiss) private var dismiss

    @State private var draft: PersonalEvent
    @FocusState private var titleFocused: Bool
    private let isNew: Bool
    /// The Task and Event switch, when the sheet is adding something new.
    private var kind: Binding<NewKind>?
    private let focusesTitle: Bool
    private var onTitle: ((String) -> Void)?

    init(event: PersonalEvent, isNew: Bool? = nil, kind: Binding<NewKind>? = nil, focusesTitle: Bool = true,
         onTitle: ((String) -> Void)? = nil) {
        self.focusesTitle = focusesTitle
        _draft = State(initialValue: event)
        self.isNew = isNew ?? event.title.isEmpty
        self.kind = kind
        self.onTitle = onTitle
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 12) {
                    VStack(alignment: .leading, spacing: 12) {
                        TextField("Event name", text: $draft.title)
                            .font(.display(22, relativeTo: .title2))
                            .foregroundStyle(Color.ink)
                            .focused($titleFocused)
                            .accessibilityIdentifier("Title")
                        RowRule()
                        HStack(spacing: 10) {
                            Image(systemName: "mappin.and.ellipse")
                                .foregroundStyle(Color.inkMuted)
                            TextField("Location", text: $draft.location)
                                .textInputAutocapitalization(.words)
                        }
                    }
                    .card(padding: 18)
                    whenCard
                    EditorCard(label: "Class") {
                        ClassChips(code: $draft.course, none: "No class", extra: [draft.course])
                        if draft.course.isEmpty {
                            RowRule().padding(.vertical, 2)
                            ColorLabelPicker(hex: $draft.color)
                        }
                    }
                    EditorCard(label: "Notes") {
                        TextField("Add notes", text: $draft.notes, axis: .vertical)
                            .lineLimit(2...8)
                    }
                    if !isNew {
                        Button("Delete Event", role: .destructive) {
                            state.removeEvent(id: draft.id)
                            dismiss()
                        }
                        .buttonStyle(.softPill)
                        .foregroundStyle(Color.danger)
                        .padding(.top, 6)
                    }
                }
                .padding(16)
            }
            .scrollDismissesKeyboard(.interactively)
            .background(Color.surface.ignoresSafeArea())
            .navigationTitle(isNew ? "New Event" : "Edit Event")
            .navigationBarTitleDisplayMode(.inline)
            .environment(\.timeZone, .gmt)
            .onAppear { if isNew && focusesTitle { titleFocused = true } }
            .onChange(of: draft.title) { _, title in onTitle?(title) }
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .tint(Color.toolbarIcon)
                }
                if let kind {
                    ToolbarItem(placement: .principal) { KindSwitch(kind: kind) }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(isNew ? "Add" : "Done") {
                        var event = draft
                        // A class decides the color, so a label picked earlier is not kept beside it.
                        if !event.course.isEmpty { event.color = "" }
                        state.save(event: event)
                        dismiss()
                    }
                    .tint(Color.brand)
                    .disabled(draft.title.trimmingCharacters(in: .whitespaces).isEmpty
                        || draft.end <= draft.start)
                }
            }
        }
    }

    /// Once on a date, or weekly on chosen days; then the hours.
    private var whenCard: some View {
        EditorCard(label: "When") {
            Picker("Repeats", selection: repeats) {
                Text("Once").tag(false)
                Text("Every week").tag(true)
            }
            .pickerStyle(.segmented)
            if draft.repeatsWeekly {
                WeekdayPicker(days: $draft.days)
            } else {
                EditorRow(label: "Date") {
                    DatePicker("Date", selection: eventDate, displayedComponents: .date)
                        .labelsHidden()
                        .accessibilityIdentifier("event-date")
                }
            }
            RowRule().padding(.vertical, 2)
            EditorRow(label: "Starts") {
                DatePicker("Starts", selection: time($draft.start), displayedComponents: .hourAndMinute)
                    .labelsHidden()
            }
            EditorRow(label: "Ends") {
                DatePicker("Ends", selection: time($draft.end), displayedComponents: .hourAndMinute)
                    .labelsHidden()
            }
            Text(draft.end <= draft.start ? "Choose an end time after the start." : Clock.duration(draft.end - draft.start))
                .font(.footnote)
                .foregroundStyle(draft.end <= draft.start ? Color.danger : Color.inkMuted)
        }
    }

    /// The schedule's classes, plus the event's own when it left the schedule.
    private var classes: [String] {
        var codes = state.schedule.courses
        if !draft.course.isEmpty, !codes.contains(draft.course) { codes.append(draft.course) }
        return codes
    }

    private var eventDate: Binding<Date> {
        Binding(
            get: { (CivilDate(draft.date) ?? state.selectedDate).instant(minute: 12 * 60, in: .gmt) },
            set: { draft.date = CivilDate($0, in: .gmt).iso })
    }

    private var repeats: Binding<Bool> {
        Binding(
            get: { draft.repeatsWeekly },
            set: { on in
                if on {
                    let letter = CivilDate(draft.date)?.letter ?? state.selectedDate.letter
                    draft.days = String(letter)
                    draft.date = ""
                } else {
                    draft.days = ""
                    draft.date = state.selectedDate.iso
                }
            })
    }

    /// Meeting minutes edited through a time picker fixed to GMT, so the value
    /// stays a campus wall-clock time rather than an instant.
    private func time(_ minutes: Binding<Int>) -> Binding<Date> {
        Binding(
            get: { Date(timeIntervalSince1970: Double(minutes.wrappedValue) * 60) },
            set: { minutes.wrappedValue = Int($0.timeIntervalSince1970.truncatingRemainder(dividingBy: 86400)) / 60 })
    }
}

/// The seven day letters as a segmented row, like Calendar's repeat editor.
struct WeekdayPicker: View {
    @Binding var days: String

    var body: some View {
        HStack(spacing: 6) {
            ForEach(Array(Weekdays.letters.enumerated()), id: \.offset) { index, letter in
                let on = days.contains(letter)
                Button {
                    if on {
                        days.removeAll { $0 == letter }
                    } else {
                        days = Weekdays.letters.filter { days.contains($0) || $0 == letter }
                    }
                } label: {
                    Text(Weekdays.shortNames[index].prefix(1))
                        .font(.display(15, relativeTo: .subheadline))
                        .frame(width: 38, height: 38)
                        .foregroundStyle(on ? Color.onBrand : Color.ink)
                        .background {
                            Circle().fill(on ? Color.brand : Color.tintFill)
                        }
                        .frame(maxWidth: .infinity, minHeight: 44)
                        .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Weekdays.names[index])
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Repeat days")
    }
}

/// The neutral event fill and the course palette as a row of swatches, the
/// way the web's detail pane offers them; empty means the neutral fill.
struct ColorLabelPicker: View {
    @Binding var hex: String

    var body: some View {
        HStack {
            Text("Color").foregroundStyle(Color.ink)
            Spacer(minLength: 4)
            HStack(spacing: 0) {
                ForEach(Array(([""] + CoursePalette.hexes).enumerated()), id: \.element) { index, swatch in
                    Button {
                        hex = swatch
                    } label: {
                        Circle()
                            .fill(CoursePalette.fill(swatch.isEmpty ? nil : swatch))
                            .frame(width: 22, height: 22)
                            .overlay {
                                Circle().strokeBorder(hex == swatch ? Color.ink : Color.hairline, lineWidth: hex == swatch ? 2 : 1)
                            }
                            .frame(width: 29, height: 36)
                            .contentShape(.rect)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(swatch.isEmpty ? "No color" : CoursePalette.names[index - 1])
                    .accessibilityAddTraits(hex == swatch ? .isSelected : [])
                }
            }
        }
        .accessibilityElement(children: .contain)
    }
}
