import Foundation
import SwiftUI

@MainActor
final class AppModel: ObservableObject {
    @Published var creds: SynapseCreds?
    @Published var searchQuery = ""
    @Published var showArchived = false
    @Published var selectedWorkspace: WorkspaceRow?
    @Published var showSessionSheet = false
    @Published var showAddWorkspace = false
    @Published var newWorkspacePath = ""
    @Published var activeSession: SynapseSession?
    @Published var chatReady = false

    let connection = SynapseConnection()
    let chatHost = ChatWebHost()

    init() {
        creds = SynapseCreds.load() ?? SynapseCreds.simulatorDefault
    }

    func boot() {
        guard let creds else { return }
        creds.save()
        connection.configure(creds: creds)
        connection.connect()
        chatHost.prepare(creds: creds)
    }

    func savePairing(host: String, port: String, token: String, tls: Bool) {
        let c = SynapseCreds(host: host, port: port, token: token, tls: tls, path: "")
        c.save()
        creds = c
        connection.disconnect()
        chatHost.reset()
        chatReady = false
        boot()
    }

    func openWorkspace(_ row: WorkspaceRow) {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        selectedWorkspace = row
        withAnimation(.spring(response: 0.38, dampingFraction: 0.86)) {
            showSessionSheet = true
        }
    }

    func openSession(_ session: SynapseSession) {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        withAnimation(.spring(response: 0.35, dampingFraction: 0.88)) {
            showSessionSheet = false
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.12) {
            withAnimation(.easeInOut(duration: 0.28)) {
                self.activeSession = session
            }
            self.chatHost.openSession(session.id)
        }
    }

    func closeChat() {
        withAnimation(.easeInOut(duration: 0.25)) {
            activeSession = nil
        }
    }

    func registerWorkspace() {
        let path = newWorkspacePath.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !path.isEmpty else { return }
        connection.registerWorkspace(path: path)
        newWorkspacePath = ""
        showAddWorkspace = false
    }

    func filteredWorkspaces() -> [WorkspaceRow] {
        let q = searchQuery.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return connection.workspacePaths().filter { row in
            guard !q.isEmpty else { return true }
            let sessions = connection.sessions(for: row.path, query: "", showArchived: showArchived)
            return row.label.lowercased().contains(q) || !sessions.isEmpty
        }
    }
}
