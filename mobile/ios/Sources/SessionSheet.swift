import SwiftUI

struct SessionSheet: View {
    @EnvironmentObject private var app: AppModel
    let workspace: WorkspaceRow
    @Environment(\.dismiss) private var dismiss

    private var sessions: [SynapseSession] {
        app.connection.sessions(for: workspace.path, query: app.searchQuery, showArchived: app.showArchived)
    }

    var body: some View {
        NavigationStack {
            List {
                if sessions.isEmpty {
                    ContentUnavailableView(
                        "No sessions",
                        systemImage: "bubble.left.and.bubble.right",
                        description: Text("Start a chat from the server or desktop")
                    )
                } else {
                    ForEach(sessions) { session in
                        Button {
                            app.openSession(session)
                        } label: {
                            SessionRowView(session: session)
                        }
                        .buttonStyle(.plain)
                        .swipeActions(edge: .trailing) {
                            Button(role: .destructive) {
                                app.connection.archive(sessionId: session.id)
                            } label: {
                                Label("Archive", systemImage: "archivebox")
                            }
                        }
                    }
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle(workspace.label)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }
}

private struct SessionRowView: View {
    let session: SynapseSession

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            Circle()
                .fill(session.state == "busy" ? Color.purple.opacity(0.85) : Color(uiColor: .systemGray4))
                .frame(width: 10, height: 10)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 4) {
                    if session.pinned {
                        Image(systemName: "star.fill")
                            .font(.caption2)
                            .foregroundStyle(.blue)
                    }
                    Text(session.displayTitle)
                        .font(.body)
                        .lineLimit(1)
                }
                Text(subtitle)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer()
        }
        .padding(.vertical, 2)
    }

    private var subtitle: String {
        if session.state == "busy" { return "Working" }
        if session.diffAdds > 0 || session.diffDels > 0 {
            return "+\(session.diffAdds) −\(session.diffDels)"
        }
        return "No changes"
    }
}
