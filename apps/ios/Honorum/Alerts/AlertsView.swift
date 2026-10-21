import SwiftUI

/// Seat alerts. Watched sections are part of the account's preferences, so the
/// list follows the student to every signed-in device; this device checks the
/// counts and announces an opening.
struct AlertsView: View {
    @Environment(AppState.self) private var state
    @State private var checking = false

    var body: some View {
        AppList {
            if state.watches.isEmpty && state.serverWatches.isEmpty {
                Section {
                    ContentUnavailableView {
                        Label("No Alerts", systemImage: "bell.slash")
                    } description: {
                        Text("Open a full class in Find Classes and turn on Watch Seat.")
                    }
                    .listRowBackground(Color.clear)
                }
            }

            if !state.watches.isEmpty {
                Section {
                    ForEach(state.watches) { watch in
                        row(watch)
                            .swipeActions {
                                Button("Stop", systemImage: "bell.slash", role: .destructive) {
                                    state.unwatch(sectionID: watch.section)
                                }
                            }
                    }
                } header: {
                    ListHeader("Watching")
                } footer: {
                    Text(
                        "Honorum checks these when you open the app and announces an opening on this device. Turn on notifications to hear about it."
                    )
                }
            }

            if !state.serverWatches.isEmpty {
                Section {
                    ForEach(state.serverWatches) { watch in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(watch.label ?? watch.section).foregroundStyle(Color.ink)
                            if let title = watch.title, !title.isEmpty {
                                Text(title).font(.caption).foregroundStyle(Color.inkMuted)
                            }
                        }
                        .swipeActions {
                            Button("Remove", systemImage: "trash", role: .destructive) {
                                Task { await state.removeServerWatch(id: watch.id) }
                            }
                        }
                    }
                } header: {
                    ListHeader("On Your Account")
                } footer: {
                    Text("Alerts created on another client. They are delivered by the server.")
                }
            }
        }
        .navigationTitle("Seat Alerts")
        .navigationBarTitleDisplayMode(.large)
        .refreshable { await check() }
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("Check Now", systemImage: "arrow.clockwise") { Task { await check() } }
                    .tint(Color.toolbarIcon)
                    .disabled(checking)
            }
        }
        .task {
            await state.refreshServerWatches()
            await check()
        }
    }

    /// A watched section, its tile green once a seat opens.
    private func row(_ watch: SeatWatch) -> some View {
        let seats = state.seatCounts[watch.section] ?? current(watch)
        let open = seats?.isOpen == true
        return HStack(spacing: 12) {
            IconTile(open ? "chair.fill" : "chair", hex: open ? Tiles.mint : Tiles.pink)
            VStack(alignment: .leading, spacing: 2) {
                Text(watch.title).foregroundStyle(Color.ink)
                Text(watch.label).font(.footnote).foregroundStyle(Color.inkMuted)
            }
            Spacer()
            if let seats {
                SeatBadge(seats: seats)
            }
        }
        .padding(.vertical, 2)
    }

    private func current(_ watch: SeatWatch) -> Seats? {
        watch.lastAvailable < 0 ? nil : Seats(capacity: 0, available: watch.lastAvailable, waitlist: 0)
    }

    private func check() async {
        checking = true
        await state.checkWatchedSeats()
        checking = false
    }
}
