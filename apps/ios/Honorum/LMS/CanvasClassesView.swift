import SwiftUI

/// Classes found in Canvas, to confirm before adding: each Canvas course
/// matched to the catalog with its sections, and a picker for each choice
/// left, like the lab a lecture needs. A class with a section is on; one
/// that is only a choice turns on once chosen, as on the web.
struct CanvasClassesView: View {
    let found: FoundClasses
    let term: String
    let add: ([(course: FoundClasses.Source, code: String, ids: [String])]) -> Void
    let skip: () -> Void

    @State private var included: [String: Bool] = [:]
    @State private var chosen: [String: [Int: String]] = [:]

    private static let kinds = [
        "LEC": "Lecture", "LAB": "Lab", "DIS": "Discussion", "REC": "Recitation",
        "SEM": "Seminar", "STU": "Studio", "TUT": "Tutorial",
    ]

    var body: some View {
        AppList {
            Section {
                ForEach(found.classes) { match in
                    Toggle(isOn: includedBinding(match)) {
                        VStack(alignment: .leading, spacing: 3) {
                            Text("\(match.code) · \(match.title)")
                            ForEach(match.sections, id: \.self) { id in
                                Text(line(id))
                                    .font(.footnote)
                                    .foregroundStyle(Color.inkSecondary)
                            }
                            if !match.sure {
                                // Canvas did not name the section; the guess is worth a look.
                                Text("Matched by instructor or a single section")
                                    .font(.caption)
                                    .foregroundStyle(Color.inkMuted)
                            }
                        }
                    }
                    ForEach(Array(match.options.enumerated()), id: \.offset) { index, group in
                        Picker("\(match.code) \(kind(of: group))", selection: choiceBinding(match, index)) {
                            Text("Choose…").tag("")
                            ForEach(group, id: \.self) { id in Text(line(id)).tag(id) }
                        }
                    }
                    if included[match.id] == true, ids(match).isEmpty {
                        Text("Choose a section to add \(match.code).")
                            .font(.caption)
                            .foregroundStyle(Color.danger)
                    }
                }
            } header: {
                ListHeader(term)
            }
            if !found.unmatched.isEmpty {
                Section {
                    ForEach(found.unmatched, id: \.id) { course in
                        Text(course.name.isEmpty ? course.code : course.name)
                            .foregroundStyle(Color.inkSecondary)
                    }
                } header: {
                    ListHeader("Not in the Catalog")
                } footer: {
                    Text("These Canvas courses have no class in \(term). Their assignments still import.")
                }
            }
        }
        .navigationTitle("Classes from Canvas")
        .navigationBarTitleDisplayMode(.inline)
        .safeAreaInset(edge: .bottom) {
            VStack(spacing: 10) {
                Button {
                    add(picks)
                } label: {
                    Text(count == 0 ? "Add Classes" : count == 1 ? "Add 1 Class" : "Add \(count) Classes")
                }
                .buttonStyle(.pill)
                .disabled(count == 0)
                Button("Skip", action: skip)
                    .buttonStyle(.softPill)
            }
            .frame(maxWidth: 420)
            .frame(maxWidth: .infinity)
            .padding(.horizontal, 28)
            .padding(.top, 12)
            .padding(.bottom, 8)
            .background(Color.surface)
        }
    }

    private func section(_ id: String) -> SavedSection? { found.sections[id].map(SavedSection.init) }

    /// "001 · Mon Wed Fri 9:00 AM – 9:50 AM"
    private func line(_ id: String) -> String {
        guard let saved = section(id) else { return id }
        return ([saved.num] + saved.meetings.map { "\(Weekdays.describe($0.days)) \(Clock.range($0.start, $0.end))" })
            .joined(separator: " · ")
    }

    /// What a choice picks: its sections' kind when they share one.
    private func kind(of group: [String]) -> String {
        let kinds = Set(group.compactMap { section($0)?.type })
        if kinds.count == 1, let only = kinds.first, let name = Self.kinds[only] { return name }
        return "Section"
    }

    private func includedBinding(_ match: FoundClasses.Match) -> Binding<Bool> {
        Binding(
            get: { included[match.id] ?? !match.sections.isEmpty },
            set: { included[match.id] = $0 })
    }

    private func choiceBinding(_ match: FoundClasses.Match, _ index: Int) -> Binding<String> {
        Binding(
            get: { chosen[match.id]?[index] ?? "" },
            set: { value in
                chosen[match.id, default: [:]][index] = value
                if !value.isEmpty { included[match.id] = true }
            })
    }

    private func ids(_ match: FoundClasses.Match) -> [String] {
        match.sections + (chosen[match.id] ?? [:]).sorted { $0.key < $1.key }.map { $0.value }.filter { !$0.isEmpty }
    }

    private var picks: [(course: FoundClasses.Source, code: String, ids: [String])] {
        found.classes.compactMap { match in
            let chosenIDs = ids(match)
            guard included[match.id] ?? !match.sections.isEmpty, !chosenIDs.isEmpty else { return nil }
            return (course: match.course, code: match.code, ids: chosenIDs)
        }
    }

    private var count: Int { picks.count }
}
