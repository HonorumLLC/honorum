import SwiftUI

/// The same setup sequence as the web, ending with the student's classes:
/// welcome, school, account, then classes. At a Canvas school the classes
/// come from Canvas: one token, and the server finds the classes and their
/// assignments together for the student to confirm. Elsewhere assignments
/// come first, and the last page opens the class search.
///
/// Each step keeps its actions in a bar at the bottom, within reach of a
/// thumb, with the step dots above them. Steps slide in from the side they
/// are reached from while the one leaving fades beneath.
struct WelcomeView: View {
    @Environment(AppState.self) private var state
    /// `-welcomePage school` opens a step for screenshots.
    @State private var page = UserDefaults.standard.string(forKey: "welcomePage").flatMap { name in
        Page.allCases.first { "\($0)" == name }
    } ?? .welcome
    /// Which way the last step change went, for the incoming step's slide.
    @State private var forward = true
    @State private var query = ""
    @State private var signingIn = false
    @State private var connecting = false
    // Classes from Canvas: ask for the token, find, confirm.
    @State private var step = Step.ask
    @State private var canvasToken = ""
    @State private var canvasError = ""
    @State private var found: FoundClasses?

    enum Page: Int, CaseIterable { case welcome, school, account, assignments, classes }
    enum Step { case ask, finding, review, nothing }

    private static let pageAnimation = Animation.smooth(duration: 0.35)
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @ScaledMetric(relativeTo: .largeTitle) private var markSize = 72.0
    @ScaledMetric(relativeTo: .title2) private var featureIcon = 32.0

    private var canSignIn: Bool { state.config?.phoneAuth == true && !state.isSignedIn }
    /// Configuration has not answered yet, so what the sequence holds is unknown.
    private var awaitingConfig: Bool { state.config == nil && state.configError == nil }
    private var canvasSchool: Bool { state.institution?.lms?.kind == "canvas" }
    /// Classes come last, since searching for them leaves the tour; where
    /// Canvas does not bring assignments with them, a page for those comes first.
    private var pages: [Page] {
        [.welcome] + ((state.config?.institutions.count ?? 0) > 1 ? [.school] : [])
            + (canSignIn ? [.account] : []) + (canvasSchool ? [] : [.assignments]) + [.classes]
    }
    private var provider: String { state.institution?.lms?.providerName ?? "Canvas" }
    private var canvasAddress: String { state.institution?.lms?.url ?? "" }
    private var foundTerm: String {
        state.institution?.terms.first { $0.id == found?.term }?.name ?? "this term"
    }
    /// The found classes carry their own Add and Skip bar.
    private var reviewing: Bool { page == .classes && step == .review }

    var body: some View {
        NavigationStack {
            ZStack {
                if page == .school {
                    schoolList
                        .transition(stepTransition)
                } else if reviewing, let found {
                    CanvasClassesView(found: found, term: foundTerm) { picks in
                        state.addFound(picks, from: found)
                        state.finishOnboarding()
                    } skip: {
                        state.finishOnboarding()
                    }
                    .transition(.opacity)
                } else {
                    ScrollView {
                        VStack(spacing: 24) {
                            header
                            if page == .welcome {
                                VStack(alignment: .leading, spacing: 18) {
                                    feature("checklist", Tiles.pink, "Track Tasks", "Add tasks or import assignments")
                                    feature("calendar", Tiles.peach, "See Your Week", "Classes, events, and what is due")
                                    feature("bell", Tiles.blue, "Seat Alerts", "Know when a full class opens")
                                }
                                .card(padding: 20)
                                // Without the configuration there is no school to
                                // choose and no sign-in to offer; say so, and offer
                                // another try rather than a shorter tour.
                                if let error = state.configError {
                                    VStack(alignment: .leading, spacing: 8) {
                                        Text(error)
                                            .font(.footnote)
                                            .foregroundStyle(Color.danger)
                                        Button("Try Again") { Task { await state.refresh() } }
                                            .buttonStyle(.softPill)
                                            .fixedSize()
                                    }
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                }
                            }
                            if page == .classes && canvasSchool && step == .ask {
                                canvasForm
                            }
                            if page == .classes && step == .finding {
                                ProgressView().controlSize(.large)
                            }
                        }
                        .frame(maxWidth: 420)
                        .frame(maxWidth: .infinity)
                        .padding(28)
                    }
                    .id(page)
                    .transition(stepTransition)
                }
            }
            .background(Color.surface)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if page != .welcome && step != .finding {
                    ToolbarItem(placement: .topBarLeading) {
                        Button("Back", systemImage: "chevron.left") { back() }
                            .tint(Color.toolbarIcon)
                    }
                }
            }
            .safeAreaInset(edge: .bottom, spacing: 0) {
                if !reviewing { footer }
            }
            .sheet(isPresented: $signingIn) { SignInView() }
            .sheet(isPresented: $connecting) {
                NavigationStack {
                    ConnectView()
                        .toolbar {
                            ToolbarItem(placement: .cancellationAction) {
                                Button("Done") { connecting = false }
                                    .tint(Color.toolbarIcon)
                            }
                        }
                }
            }
            .onChange(of: state.isSignedIn) { _, signedIn in
                if signedIn && page == .account { next() }
            }
            // A connected calendar ends the assignments page; its tasks are on the way.
            .onChange(of: state.assignmentsConnected) { _, connected in
                if connected && page == .assignments {
                    connecting = false
                    next()
                }
            }
            .onChange(of: page) { _, page in
                if page == .assignments && state.assignmentsConnected { next() }
            }
        }
    }

    private var symbol: String {
        switch page {
        case .welcome: return ""
        case .school: return "mappin"
        case .account: return "person.crop.circle"
        case .assignments: return "arrow.trianglehead.2.clockwise"
        case .classes: return "books.vertical"
        }
    }

    private var title: String {
        switch page {
        case .welcome: return "Welcome to Honorum"
        case .school: return "Choose Your School"
        case .account: return "Sync Across Devices"
        case .assignments: return "Import Assignments"
        case .classes:
            if step == .finding { return "Finding Your Classes" }
            if step == .nothing { return "No Classes Found" }
            return "Add Your Classes"
        }
    }

    private var detail: String {
        switch page {
        case .account: return "Keep schedules and tasks on every device."
        case .assignments: return "Bring due dates from \(provider) or a calendar feed."
        case .classes where step == .ask && !canvasSchool: return "Search your school’s catalog and build a schedule."
        default: return ""
        }
    }

    private var header: some View {
        VStack(spacing: 16) {
            if page == .welcome {
                // The planner's three rings, closing around the mark.
                ZStack {
                    RingDial(counts: [
                        .today: TaskCount(done: 4, total: 5), .week: TaskCount(done: 3, total: 5),
                        .term: TaskCount(done: 2, total: 5),
                    ], size: markSize * 1.9, stroke: markSize * 0.15, gap: markSize * 0.04)
                    Image(.brandMark).resizable().renderingMode(.template)
                        .scaledToFit().frame(width: markSize * 0.62, height: markSize * 0.62)
                        .foregroundStyle(Color.brand)
                }
                .padding(.bottom, 6)
            } else {
                Image(systemName: symbol)
                    .font(.system(size: markSize * 0.42, weight: .semibold))
                    .foregroundStyle(CoursePalette.mark(tile))
                    .frame(width: markSize, height: markSize)
                    .background(CoursePalette.fill(tile), in: .rect(cornerRadius: markSize * 0.3, style: .continuous))
            }
            Text(title)
                .font(.display(30, relativeTo: .largeTitle))
                .foregroundStyle(Color.ink)
                .multilineTextAlignment(.center)
            if !detail.isEmpty {
                Text(detail)
                    .foregroundStyle(Color.inkSecondary)
                    .multilineTextAlignment(.center)
            }
        }
        .padding(.top, 24)
    }

    /// Each step's tile color, from the class palette.
    private var tile: String {
        switch page {
        case .welcome, .school: Tiles.periwinkle
        case .account: Tiles.yellow
        case .assignments: Tiles.mint
        case .classes: Tiles.blue
        }
    }

    private func feature(_ symbol: String, _ hex: String, _ title: String, _ detail: String) -> some View {
        HStack(spacing: 14) {
            Image(systemName: symbol)
                .font(.system(size: featureIcon * 0.5, weight: .semibold))
                .foregroundStyle(CoursePalette.mark(hex))
                .frame(width: featureIcon * 1.25, height: featureIcon * 1.25)
                .background(CoursePalette.fill(hex), in: .rect(cornerRadius: featureIcon * 0.38, style: .continuous))
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.headline).foregroundStyle(Color.ink)
                Text(detail).font(.subheadline).foregroundStyle(Color.inkSecondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// The token, asked for in place.
    private var canvasForm: some View {
        VStack(alignment: .leading, spacing: 12) {
            if !state.canvasConnected {
                SecureField("Canvas access token", text: $canvasToken)
                    .textFieldStyle(.roundedBorder)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                if !canvasAddress.isEmpty, let settings = URL(string: canvasAddress + "/profile/settings") {
                    Link(destination: settings) {
                        Label("Get a Token", systemImage: "arrow.up.right.square")
                    }
                    .font(.subheadline)
                }
            }
            if !canvasError.isEmpty {
                Text(canvasError)
                    .font(.footnote)
                    .foregroundStyle(Color.danger)
            }
        }
    }

    private var schoolList: some View {
        let schools = (state.config?.institutions ?? []).filter {
            query.isEmpty || $0.name.localizedCaseInsensitiveContains(query)
        }
        return AppList {
            if schools.isEmpty {
                ContentUnavailableView.search(text: query)
                    .listRowBackground(Color.clear)
            }
            Section {
                ForEach(schools) { school in
                    Button {
                        state.choose(institution: school.id)
                        next()
                    } label: {
                        HStack {
                            TileLabel(school.name, "building.columns", hex: Tiles.periwinkle)
                            Spacer()
                            Image(systemName: "chevron.right").foregroundStyle(Color.inkMuted)
                        }
                    }
                }
            } header: {
                ListHeader("Choose Your School")
            }
        }
        .searchable(text: $query, prompt: "Search schools")
    }

    /// The incoming step slides in from the side it is reached from; the one
    /// leaving fades where it is, so it needs no knowledge of the direction.
    private var stepTransition: AnyTransition {
        guard !reduceMotion else { return .opacity }
        return .asymmetric(
            insertion: .move(edge: forward ? .trailing : .leading).combined(with: .opacity),
            removal: .opacity)
    }

    /// The step animation, or none when Reduce Motion is on.
    private var pageAnimation: Animation? { reduceMotion ? nil : Self.pageAnimation }

    /// The step dots and, on every step but the school list, the actions.
    /// Changes here animate, including the Sign In option and the dots that
    /// arrive with the configuration, rather than popping in.
    private var footer: some View {
        VStack(spacing: 16) {
            progressDots
            if page != .school { actions }
        }
        .frame(maxWidth: 420)
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 28)
        .padding(.top, 12)
        .padding(.bottom, 8)
        .background(Color.surface)
        .animation(pageAnimation, value: page)
        .animation(pageAnimation, value: step)
        .animation(pageAnimation, value: pages)
        .animation(pageAnimation, value: canSignIn)
    }

    /// One dot per step; the current one stretches, the way page controls do.
    private var progressDots: some View {
        let index = pages.firstIndex(of: page) ?? 0
        return HStack(spacing: 8) {
            ForEach(Array(pages.enumerated()), id: \.offset) { position, _ in
                Capsule()
                    .fill(position == index ? Color.brand : Color.hairline)
                    .frame(width: position == index ? 22 : 8, height: 8)
            }
        }
        .animation(reduceMotion ? nil : .snappy(duration: 0.25), value: page)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Setup progress")
        .accessibilityValue("Step \(index + 1) of \(pages.count)")
    }

    /// The primary action filled, the secondary bordered, both the same size.
    /// While Canvas is searched there is nothing to press but Back.
    @ViewBuilder private var actions: some View {
        if let primary {
            VStack(spacing: 10) {
                Button(primary.title, action: primary.action)
                    .buttonStyle(.pill)
                    .disabled(awaitingConfig || primary.disabled)
                if let secondary {
                    Button(secondary.title, action: secondary.action)
                        .buttonStyle(.softPill)
                        .transition(.opacity)
                } else if page == .welcome && awaitingConfig {
                    // Sign In may arrive with the configuration: hold its room so
                    // the bar does not grow under a thumb.
                    Button("Sign In") {}
                        .buttonStyle(.softPill)
                        .hidden()
                }
            }
        }
    }

    private var primary: (title: String, disabled: Bool, action: () -> Void)? {
        switch page {
        case .welcome: ("Get Started", false, { next() })
        case .school: ("Get Started", false, { next() })
        case .account: ("Sign In", false, { signingIn = true })
        case .assignments:
            (state.institution?.lms == nil ? "Connect Assignments" : "Connect \(provider)", false, { connecting = true })
        case .classes:
            switch step {
            case .ask where canvasSchool:
                ("Find Classes",
                 !state.canvasConnected && canvasToken.trimmingCharacters(in: .whitespacesAndNewlines).count < 20,
                 { Task { await findClasses() } })
            case .ask, .nothing: ("Search Classes", false, { search() })
            case .finding, .review: nil
            }
        }
    }

    private var secondary: (title: String, action: () -> Void)? {
        switch page {
        case .welcome where canSignIn: ("Sign In", { signingIn = true })
        case .account: ("Continue Without an Account", { next() })
        case .assignments: ("Skip", { next() })
        case .classes where step == .nothing: ("Done", { state.finishOnboarding() })
        case .classes where step == .ask: ("Skip", { state.finishOnboarding() })
        default: nil
        }
    }

    /// Move to a step, sliding it in from the side it lies on.
    private func show(_ target: Page) {
        guard target != page else { return }
        forward = target.rawValue > page.rawValue
        withAnimation(pageAnimation) { page = target }
    }

    private func next() {
        show(pages.first(where: { $0.rawValue > page.rawValue }) ?? .classes)
    }

    private func back() {
        if page == .classes && step != .ask {
            withAnimation(pageAnimation) { step = .ask }
        } else {
            show(pages.last(where: { $0.rawValue < page.rawValue }) ?? .welcome)
        }
    }

    /// The class search takes over once the tour is done.
    private func search() {
        state.editClassesOnOpen = true
        state.finishOnboarding()
    }

    private func findClasses() async {
        canvasError = ""
        withAnimation(pageAnimation) { step = .finding }
        do {
            if !state.canvasConnected {
                try await state.connectCanvas(
                    base: canvasAddress, token: canvasToken.trimmingCharacters(in: .whitespacesAndNewlines))
            }
            canvasToken = ""
            let result = try await state.findCanvasClasses()
            found = result
            withAnimation(pageAnimation) { step = result.classes.isEmpty ? .nothing : .review }
        } catch {
            withAnimation(pageAnimation) { step = .ask }
            let message = (error as? LocalizedError)?.errorDescription ?? "Canvas could not be reached."
            canvasError = message.localizedCaseInsensitiveContains("rejected the token")
                ? "Canvas rejected that token. Generate a new one and try again."
                : message
        }
    }
}

#if DEBUG
#Preview("Welcome") {
    WelcomeView()
        .environment(AppState.preview())
        .tint(.brand)
}
#endif
