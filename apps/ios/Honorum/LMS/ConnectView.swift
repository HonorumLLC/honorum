import SwiftUI

/// Account Canvas connections are shared; guest tokens and feeds stay local.
/// Each section reports its own failure beneath its own controls, and a
/// failed action is cleared by the next edit to that section.
struct ConnectView: View {
    @Environment(AppState.self) private var state

    private enum Field: Hashable { case address, token, feed }

    @State private var canvasAddress = ""
    @State private var canvasToken = ""
    @State private var feed = ""
    @State private var working = ""
    @State private var canvasError = ""
    @State private var feedError = ""
    @State private var disconnecting = false
    @State private var removingFeed: String?
    @FocusState private var focus: Field?

    private var provider: String { state.institution?.lms?.providerName ?? "Canvas" }
    private var trimmedToken: String { canvasToken.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var trimmedFeed: String { feed.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        AppList {
            canvasSection
            feedSection
            Section {
                LabeledContent {
                    Text(statusText)
                        .foregroundStyle(statusFailed ? Color.danger : Color.inkMuted)
                        .multilineTextAlignment(.trailing)
                } label: {
                    TileLabel("Status", "arrow.triangle.2.circlepath", hex: Tiles.neutral)
                }
                Button("Refresh Now") { Task { await state.refreshAssignments(force: true) } }
                    .disabled(!state.assignmentsConnected || state.accountCanvasRefreshing)
            } header: {
                ListHeader("Import")
            }
        }
        .scrollDismissesKeyboard(.interactively)
        .navigationTitle("Assignment Sources")
        .navigationBarTitleDisplayMode(.large)
        .onAppear { canvasAddress = state.canvasBase.isEmpty ? suggestedAddress : state.canvasBase }
        .confirmationDialog("Disconnect \(provider)?", isPresented: $disconnecting, titleVisibility: .visible) {
            Button("Disconnect", role: .destructive) { Task { await state.disconnectCanvas() } }
        } message: {
            Text(state.accountCanvas?.connected == true
                 ? "\(provider) disconnects on every device. Existing assignments stay saved."
                 : "\(provider) assignments will be removed from this device.")
        }
        .confirmationDialog(
            "Remove this calendar feed?",
            isPresented: Binding(get: { removingFeed != nil }, set: { if !$0 { removingFeed = nil } }),
            titleVisibility: .visible, presenting: removingFeed
        ) { url in
            Button("Remove Feed", role: .destructive) { state.removeFeed(url: url) }
        } message: { url in
            Text(url)
        }
    }

    private var suggestedAddress: String { state.institution?.lms?.url ?? "" }

    @ViewBuilder private var canvasSection: some View {
        if state.canvasConnected {
            Section {
                LabeledContent("Address") {
                    Text(state.accountCanvas?.connected == true ? state.accountCanvas?.base ?? "" : state.canvasBase)
                        .multilineTextAlignment(.trailing)
                }
                if state.isSignedIn && state.accountCanvas?.connected != true {
                    Button("Use \(provider) on All Devices") {
                        Task {
                            working = "canvas"
                            canvasError = ""
                            defer { working = "" }
                            do { try await state.connectCanvas(base: state.canvasBase, token: Keychain.read(.canvasToken) ?? "") }
                            catch { canvasError = (error as? LocalizedError)?.errorDescription ?? "Could not connect \(provider)." }
                        }
                    }
                    .disabled(!working.isEmpty)
                    Text("Saves an encrypted token with your account.")
                        .font(.footnote)
                        .foregroundStyle(Color.inkMuted)
                }
                LabeledContent("Assignments", value: "\(count(of: .canvas))")
                if state.accountCanvas?.connected == true {
                    LabeledContent("Available On", value: "All devices")
                }
                Button("Disconnect \(provider)", role: .destructive) { disconnecting = true }
            } header: {
                ListHeader(provider)
            } footer: {
                failure(canvasError)
            }
        } else {
            Section {
                TextField("https://canvas.yourschool.edu", text: $canvasAddress)
                    .keyboardType(.URL)
                    .textContentType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .focused($focus, equals: .address)
                    .submitLabel(.next)
                    .onSubmit { focus = .token }
                    .onChange(of: canvasAddress) { canvasError = "" }
                // A token is a secret but not a password, so it asks for no
                // saved-password AutoFill.
                SecureField("Access token", text: $canvasToken)
                    .textContentType(.oneTimeCode)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .focused($focus, equals: .token)
                    .submitLabel(.go)
                    .onSubmit { if canConnect { Task { await connectCanvas() } } }
                    .onChange(of: canvasToken) { canvasError = "" }
                Button {
                    Task { await connectCanvas() }
                } label: {
                    if working == "canvas" {
                        HStack { ProgressView(); Text("Connecting…") }
                    } else {
                        Text("Connect \(provider)")
                    }
                }
                .disabled(!canConnect)
            } header: {
                ListHeader(provider)
            } footer: {
                VStack(alignment: .leading, spacing: 6) {
                    Text(
                        "In \(provider): Account ▸ Settings ▸ New Access Token. " + (state.isSignedIn ? "Connecting saves an encrypted token with your account for all your devices." : "The token stays on this device.")
                    )
                    failure(canvasError)
                }
            }
        }
    }

    private var canConnect: Bool {
        !canvasAddress.trimmingCharacters(in: .whitespaces).isEmpty && !trimmedToken.isEmpty && working.isEmpty
    }

    private var feedSection: some View {
        Section {
            ForEach(state.accountFeeds) { feed in
                VStack(alignment: .leading) {
                    Text(feed.label.isEmpty ? "Calendar" : feed.label)
                    Text(feed.error.isEmpty ? "On all your devices" : feed.error)
                        .font(.footnote)
                        .foregroundStyle(feed.error.isEmpty ? Color.inkMuted : Color.danger)
                }
                .swipeActions(allowsFullSwipe: false) {
                    Button("Remove", systemImage: "trash", role: .destructive) {
                        Task { await state.removeAccountFeed(id: feed.id) }
                    }
                }
                .contextMenu {
                    Button("Remove", systemImage: "trash", role: .destructive) {
                        Task { await state.removeAccountFeed(id: feed.id) }
                    }
                }
            }
            ForEach(state.feeds, id: \.self) { url in
                Text(url)
                    .font(.footnote)
                    .lineLimit(3)
                    .swipeActions(allowsFullSwipe: false) {
                        Button("Remove", systemImage: "trash", role: .destructive) { removingFeed = url }
                    }
                    .contextMenu {
                        Button("Remove", systemImage: "trash", role: .destructive) { removingFeed = url }
                    }
            }
            TextField("https://… .ics", text: $feed)
                .keyboardType(.URL)
                .textContentType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .focused($focus, equals: .feed)
                .submitLabel(.go)
                .onSubmit { if !trimmedFeed.isEmpty, working.isEmpty { Task { await addFeed() } } }
                .onChange(of: feed) { feedError = "" }
            Button {
                Task { await addFeed() }
            } label: {
                if working == "feed" {
                    HStack { ProgressView(); Text("Checking…") }
                } else {
                    Text("Add Calendar Feed")
                }
            }
            .disabled(trimmedFeed.isEmpty || !working.isEmpty)
        } header: {
            ListHeader("Calendar Feeds")
        } footer: {
            VStack(alignment: .leading, spacing: 6) {
                Text(
                    "In Canvas: Calendar ▸ Calendar Feed. " + (state.isSignedIn ? "A link is saved encrypted with your account, so every device gets its assignments." : "The link stays on this device.")
                )
                failure(feedError)
            }
        }
    }

    @ViewBuilder private func failure(_ message: String) -> some View {
        if !message.isEmpty {
            Text(message)
                .foregroundStyle(Color.danger)
                .accessibilityAddTraits(.isStaticText)
        }
    }

    private var statusFailed: Bool {
        if case .failed = state.importStatus { return true }
        return false
    }

    private var statusText: String {
        switch state.importStatus {
        case .idle: state.assignmentsConnected ? "Ready" : "Nothing connected"
        case .importing(let what): "Importing \(what)…"
        case .failed(let message): message
        case .finished(let when): "Updated \(when.formatted(.relative(presentation: .named)))"
        }
    }

    private func count(of source: StudentTask.Source) -> Int {
        state.library.tasks.filter { $0.source == source }.count
    }

    private func connectCanvas() async {
        working = "canvas"
        canvasError = ""
        defer { working = "" }
        do {
            try await state.connectCanvas(base: canvasAddress, token: trimmedToken)
            canvasToken = ""
        } catch {
            canvasError = (error as? LocalizedError)?.errorDescription
                ?? "\(provider) could not be reached with those details."
            AccessibilityNotification.Announcement(canvasError).post()
        }
    }

    private func addFeed() async {
        working = "feed"
        feedError = ""
        defer { working = "" }
        do {
            try await state.addFeed(url: trimmedFeed)
            feed = ""
        } catch {
            feedError = (error as? LocalizedError)?.errorDescription
                ?? "That link is not a calendar feed."
            AccessibilityNotification.Announcement(feedError).post()
        }
    }
}

/// Matching an imported course to a class in the schedule, once.
struct CourseMatchView: View {
    @Environment(AppState.self) private var state

    var body: some View {
        AppList {
            Section {
                ForEach(state.importedCourses, id: \.id) { course in
                    Picker(course.name, selection: binding(for: course.courseID, connection: course.connection)) {
                        Text("Leave as is").tag("")
                        ForEach(codes, id: \.self) { code in Text(code).tag(code) }
                    }
                }
            }
        }
        .navigationTitle("Canvas Courses")
        .navigationBarTitleDisplayMode(.large)
    }

    private var codes: [String] {
        Array(Set(state.library.schedule.sections.map(\.course))).sorted()
    }

    private func binding(for id: String, connection: String) -> Binding<String> {
        Binding(
            get: { state.mappedCourse(for: id, connection: connection) },
            set: { state.map(lmsCourse: id, to: $0, connection: connection) })
    }
}
