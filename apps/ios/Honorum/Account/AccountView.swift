import SwiftUI

/// Who is signed in, their sync, and their streak, at the top of Account;
/// for a guest, the offer to sign in beside the planner's own rings.
struct ProfileSection: View {
    @Environment(AppState.self) private var state
    @State private var signingIn = false

    var body: some View {
        Section {
            if state.isSignedIn {
                HStack(spacing: 14) {
                    Image(systemName: "person.fill")
                        .font(.system(size: 22, weight: .semibold))
                        .foregroundStyle(Color.brandInk)
                        .frame(width: 52, height: 52)
                        .background(Color.tintFill, in: .circle)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(state.account?.phone ?? "Signed in")
                            .font(.display(19, relativeTo: .headline))
                            .foregroundStyle(Color.ink)
                        HStack(spacing: 6) {
                            Circle().fill(syncColor).frame(width: 7, height: 7)
                            Text(syncSummary)
                                .font(.subheadline)
                                .foregroundStyle(Color.inkSecondary)
                        }
                    }
                }
                .padding(.vertical, 6)
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("sync-status")
                StreakPage(streak: state.streak, signedIn: true, canSignIn: state.config?.phoneAuth != false,
                           now: Date(), zone: state.timeZone) {}
                    .padding(.vertical, 8)
            } else {
                VStack(alignment: .leading, spacing: 16) {
                    HStack(spacing: 14) {
                        RingDial(counts: counts, size: 52, stroke: 4.5, gap: 1.5)
                        VStack(alignment: .leading, spacing: 3) {
                            Text("Sign in to Honorum")
                                .font(.display(19, relativeTo: .headline))
                                .foregroundStyle(Color.ink)
                            Text("Sync your plan, keep a streak, and get seat alerts.")
                                .font(.subheadline)
                                .foregroundStyle(Color.inkSecondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    Button("Sign In") { signingIn = true }
                        .buttonStyle(.pill)
                        .disabled(state.config?.phoneAuth == false)
                }
                .padding(.vertical, 10)
            }
        } footer: {
            if case .failed(let message) = state.settledSyncStatus {
                Text(message).foregroundStyle(Color.danger)
            } else if state.config?.phoneAuth == false && !state.isSignedIn {
                Text("Sign-in is unavailable.")
            }
        }
        .sheet(isPresented: $signingIn) { SignInView() }
    }

    private var counts: [RingPeriod: TaskCount] {
        let rings = state.ringCounts(now: Date())
        return Dictionary(uniqueKeysWithValues: RingPeriod.allCases.map {
            ($0, TaskCount(done: rings[$0].done, total: rings[$0].total))
        })
    }

    private var syncSummary: String {
        switch state.settledSyncStatus {
        case .failed: "Not synced"
        case .synced: "Synced"
        case .idle, .syncing: "Waiting to sync"
        }
    }

    private var syncColor: Color {
        switch state.settledSyncStatus {
        case .failed: Color.danger
        case .synced: Color.success
        case .idle, .syncing: Color.inkMuted
        }
    }
}

/// The signed-in account's devices and sync, after the planner's settings.
struct SyncSection: View {
    @Environment(AppState.self) private var state
    /// Only a sync the student asked for shows progress. Automatic exchanges
    /// run every minute and after every live update; reporting each one just
    /// makes the account look busy.
    @State private var syncingManually = false

    var body: some View {
        if state.isSignedIn {
            Section {
                NavigationLink(value: ClassesRoute.devices) {
                    LabeledContent {
                        Text("\(state.account?.devices.count ?? 1)")
                    } label: {
                        TileLabel("Devices", "laptopcomputer.and.iphone", hex: Tiles.yellow)
                    }
                }
                if !state.syncState.conflicts.isEmpty {
                    NavigationLink(value: ClassesRoute.copies) {
                        TileLabel("Recover Conflicting Edits", "exclamationmark.arrow.triangle.2.circlepath", hex: Tiles.yellow)
                    }
                }
                Button {
                    Task {
                        syncingManually = true
                        await state.syncNow()
                        syncingManually = false
                    }
                } label: {
                    LabeledContent {
                        if syncingManually { ProgressView() }
                    } label: {
                        TileLabel("Sync Now", "arrow.triangle.2.circlepath", hex: Tiles.yellow)
                    }
                }
                .disabled(syncingManually)
            } header: {
                ListHeader("Sync")
            }
        }
    }
}

/// Signing out and deleting the account, at the end of the Account tab with
/// the plan reset, where the Human Interface Guidelines put what cannot be
/// taken back.
struct AccountDangerRows: View {
    @Environment(AppState.self) private var state
    @State private var confirmingSignOut = false
    @State private var confirmingDelete = false
    @State private var deleting = false

    var body: some View {
        if state.isSignedIn {
            Button("Sign Out") { confirmingSignOut = true }
            Button {
                confirmingDelete = true
            } label: {
                HStack {
                    Text("Delete Account")
                    if deleting {
                        Spacer()
                        ProgressView()
                    }
                }
            }
            .foregroundStyle(Color.danger)
            .disabled(deleting)
            .confirmationDialog(
                "Sign out of Honorum?", isPresented: $confirmingSignOut, titleVisibility: .visible
            ) {
                // Not destructive: nothing is lost, and the plan returns on the next sign-in.
                Button("Sign Out") { Task { await state.signOut() } }
            } message: {
                Text("Your plan stays with your account and comes back when you sign in again.")
            }
            .confirmationDialog(
                "Delete this account?", isPresented: $confirmingDelete, titleVisibility: .visible
            ) {
                Button("Delete Account", role: .destructive) {
                    Task {
                        deleting = true
                        defer { deleting = false }
                        do { try await state.deleteAccount() } catch {
                            state.report("Could Not Delete Account", error)
                        }
                    }
                }
            } message: {
                Text(
                    "This removes the account’s schedules, tasks, alerts, and connected Canvas account. This device keeps its plan."
                )
            }
        }
    }
}

struct DevicesView: View {
    @Environment(AppState.self) private var state
    @State private var renaming = false
    @State private var newName = ""
    @State private var removing: Account.Device?
    @State private var loading = false

    var body: some View {
        AppList {
            Section {
                if loading && (state.account?.devices ?? []).isEmpty {
                    ProgressView("Loading devices")
                }
                ForEach(state.account?.devices ?? []) { device in
                    HStack(spacing: 12) {
                        IconTile(symbol(device), hex: device.current ? Tiles.yellow : Tiles.neutral)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(device.name).foregroundStyle(Color.ink)
                            Text(
                                device.current
                                    ? "This device"
                                    : "Last used \(device.lastSeenDate.formatted(.relative(presentation: .named)))"
                            )
                            .font(.caption).foregroundStyle(Color.inkMuted)
                        }
                        Spacer()
                        if !device.current {
                            Button("Remove", role: .destructive) { removing = device }
                                .buttonStyle(.borderless)
                        }
                    }
                }
            } header: {
                ListHeader("Signed In")
            }
            Section {
                Button("Rename This Device") {
                    newName =
                        state.account?.devices.first(where: \.current)?.name ?? state.deviceName
                    renaming = true
                }
            }
        }
        .navigationTitle("Devices")
        .navigationBarTitleDisplayMode(.large)
        .task {
            loading = true
            await state.refreshAccount()
            loading = false
        }
        .alert("Rename Device", isPresented: $renaming) {
            TextField("Name", text: $newName)
            Button("Cancel", role: .cancel) {}
            Button("Save") { Task { await state.rename(device: newName) } }
                .disabled(newName.trimmingCharacters(in: .whitespaces).isEmpty)
        }
        .confirmationDialog(
            "Remove device?",
            isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
            titleVisibility: .visible, presenting: removing
        ) { device in
            Button("Remove", role: .destructive) { Task { await state.revoke(device: device.id) } }
        } message: { device in
            Text("\(device.name) will be signed out.")
        }
    }
}

extension DevicesView {
    /// The kind of device, guessed from the name it reports.
    fileprivate func symbol(_ device: Account.Device) -> String {
        let name = device.name.lowercased()
        if name.contains("ipad") { return "ipad" }
        if name.contains("mac") || name.contains("book") || name.contains("windows") || name.contains("chrome") { return "laptopcomputer" }
        if name.contains("android") || name.contains("iphone") || name.contains("phone") { return "iphone" }
        return "desktopcomputer"
    }
}

/// Remote versions that could not be merged, kept so nothing is lost.
struct ConflictsView: View {
    @Environment(AppState.self) private var state

    var body: some View {
        AppList {
            if state.syncState.conflicts.isEmpty {
                ContentUnavailableView {
                    Label("Nothing to Recover", systemImage: "checkmark.circle")
                } description: {
                    Text("Every edit from your other devices has been merged.")
                }
                .listRowBackground(Color.clear)
            }
            ForEach(state.syncState.conflicts) { copy in
                Section {
                    if ["schedule", "task"].contains(copy.kind), copy.body != .object([:]) {
                        Button("Restore Copy") { state.restoreConflict(id: copy.id) }
                    } else {
                        Text("Settings changed on another device.").foregroundStyle(
                            Color.inkSecondary)
                    }
                    Button("Discard This Copy", role: .destructive) {
                        state.discardConflict(id: copy.id)
                    }
                } header: {
                    ListHeader(copy.label)
                } footer: {
                    Text("Saved \(copy.at.formatted(date: .abbreviated, time: .shortened))")
                }
            }
        }
        .navigationTitle("Saved Copies")
        .navigationBarTitleDisplayMode(.large)
    }
}
