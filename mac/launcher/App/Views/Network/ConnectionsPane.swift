import PaddockKit
import SwiftUI

/// The `Connections` part of the Grants tab: the recent connections of
/// the session with their verdict, and one click to allow a denied host or
/// its whole site.
@MainActor
struct ConnectionsPane: View {
    let net: NetworkModel
    @State private var filter: ConnectionFilter = .all

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Picker("Show", selection: $filter) {
                    ForEach(ConnectionFilter.allCases) { item in Text(item.title).tag(item) }
                }
                .pickerStyle(.segmented)
                .frame(maxWidth: 360)
                .accessibilityIdentifier("connections-filter")
                Spacer()
                if net.lastSession {
                    Text("From the last session").font(.callout).labelInk()
                }
                Button("Refresh") { Task { await net.loadConnections(force: true) } }
                    .accessibilityIdentifier("connections-refresh")
            }
            if let problem = net.problem {
                NetProblemBanner(message: problem) { net.problem = nil }
            }
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .padding()
        // Loads at once, then every 10 s while the list is on screen.
        .task { await net.followConnections() }
    }

    @ViewBuilder
    private var content: some View {
        let shown = net.connections.filter { filter.matches($0) }
        switch net.connectionsState {
        case .loaded where net.connections.isEmpty:
            PlainEmpty {
                Label { Text("No connections yet") } icon: { DecorativeSymbol(name: "network") }
            } description: {
                Text("The servers the app connects to show here while it runs, with what paddock decided.")
            } actions: {
                EmptyView()
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel(Text("No connections yet"))
            .accessibilityIdentifier("connections-empty")
        case .loaded where shown.isEmpty:
            PlainEmpty {
                Label { Text("Nothing matches the filter") } icon: { DecorativeSymbol(name: "line.3.horizontal.decrease.circle") }
            } description: {
                EmptyView()
            } actions: {
                Button("Show All") { filter = .all }
            }
        case .loaded:
            list(shown)
        default:
            if net.connections.isEmpty || net.connectionsState == .offline || net.connectionsState == .needsNewerPaddock {
                NetStateView(state: net.connectionsState, subject: .connections) {
                    Task { await net.loadConnections(force: true) }
                }
            } else {
                list(shown)
            }
        }
    }

    private func list(_ shown: [ConnectionJSON]) -> some View {
        TimelineView(.periodic(from: .now, by: 30)) { context in
            List(shown) { connection in
                ConnectionRow(net: net, connection: connection, now: context.date)
            }
            .accessibilityIdentifier("connections-list")
        }
    }
}

/// One connection: the verdict, host and port, the process, how often and
/// when. A denied row offers `Allow` and `Allow <site>`.
@MainActor
struct ConnectionRow: View {
    let net: NetworkModel
    let connection: ConnectionJSON
    let now: Date
    @State private var working = false

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: ConnectionCopy.symbol(connection))
                .foregroundStyle(tint)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: ConnectionCopy.target(connection))
                    .labelInk()
                    .textSelection(.enabled)
                if let process = connection.process {
                    Text(verbatim: process).font(.caption).labelInk()
                }
                Text(verbatim: summary).font(.caption).labelInk()
                if let done = net.allowed[connection.id] {
                    Text(done)
                        .font(.caption)
                        .accessibilityIdentifier("connection-allowed")
                }
            }
            Spacer(minLength: 8)
            if connection.isDenied, net.allowed[connection.id] == nil {
                Button("Allow") { allow(site: false) }
                    .help(Text(UIText("Always allow %@ on port %@", connection.host, "\(connection.port)")))
                    .accessibilityLabel(Text(UIText("Always allow %@ on port %@", connection.host, "\(connection.port)")))
                    .accessibilityIdentifier("allow-host-" + connection.host)
                if let site = NetSite.registrable(connection.host) {
                    Button { allow(site: true) } label: { Text(UIText("Allow %@", site)) }
                        .help(Text(UIText("Allow %@ and all its subdomains", site)))
                        .accessibilityLabel(Text(UIText("Allow %@ and all its subdomains", site)))
                        .accessibilityIdentifier("allow-site-" + connection.host)
                }
            }
        }
        .disabled(working)
        .padding(.vertical, 2)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("connection-" + connection.host)
    }

    private var tint: Color {
        switch connection.verdict {
        case "allow": return .green
        case "deny": return .red
        default: return .orange
        }
    }

    /// `Denied · 5 times · 3 min ago · No answer in time`.
    private var summary: String {
        var parts = [ConnectionCopy.verdict(connection).localized, ConnectionCopy.times(connection).localized,
                     Format.age(since: connection.seen, now: now).localized]
        if let reason = ConnectionCopy.reason(connection) { parts.append(reason.localized) }
        return parts.joined(separator: " · ")
    }

    private func allow(site: Bool) {
        working = true
        Task {
            await net.allow(connection, site: site)
            working = false
        }
    }
}
