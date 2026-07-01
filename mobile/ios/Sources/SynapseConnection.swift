import Foundation
import Combine

/// Native WebSocket client for workspace/session shell (separate from chat webview WS).
@MainActor
final class SynapseConnection: ObservableObject {
    @Published private(set) var connected = false
    @Published private(set) var sessions: [SynapseSession] = []
    @Published private(set) var cwds: [String] = []
    @Published var lastError: String?

    private var creds: SynapseCreds?
    private var task: URLSessionWebSocketTask?
    private var session: URLSession?
    private var receiveActive = false

    func configure(creds: SynapseCreds) {
        self.creds = creds
    }

    func connect() {
        guard let creds, let url = creds.wsURL else {
            lastError = "Invalid server URL"
            return
        }
        disconnect()
        let config = URLSessionConfiguration.default
        session = URLSession(configuration: config)
        task = session?.webSocketTask(with: url)
        receiveActive = true
        task?.resume()
        connected = true
        receiveLoop()
        send(["op": "list"])
    }

    func disconnect() {
        receiveActive = false
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
        session?.invalidateAndCancel()
        session = nil
        connected = false
    }

    func refresh() {
        send(["op": "refresh"])
        send(["op": "refresh_cwds"])
    }

    func registerWorkspace(path: String) {
        send(["op": "register_project", "path": path])
    }

    func archive(sessionId: String) {
        send(["op": "archive", "sessionId": sessionId])
    }

    func workspacePaths() -> [WorkspaceRow] {
        var paths = Set<String>()
        for s in sessions {
            let p = normalizePath(s.cwd)
            if !p.isEmpty { paths.insert(p) }
        }
        for p in cwds {
            let n = normalizePath(p)
            if !n.isEmpty { paths.insert(n) }
        }
        let latest: (String) -> UInt64 = { path in
            sessions.filter { normalizePath($0.cwd) == path }
                .map(\.startedAt).max() ?? 0
        }
        return paths.sorted { a, b in
            let d = latest(b) - latest(a)
            if d != 0 { return d > 0 }
            return a.localizedCaseInsensitiveCompare(b) == .orderedAscending
        }.map { WorkspaceRow(path: $0) }
    }

    func sessions(for workspace: String, query: String, showArchived: Bool) -> [SynapseSession] {
        let ws = normalizePath(workspace)
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return sessions
            .filter { showArchived ? $0.archived : !$0.archived }
            .filter { normalizePath($0.cwd) == ws }
            .filter { s in
                guard !q.isEmpty else { return true }
                return s.displayTitle.lowercased().contains(q)
                    || s.workspaceLabel.lowercased().contains(q)
            }
            .sorted { a, b in
                if a.pinned != b.pinned { return a.pinned && !b.pinned }
                return a.startedAt > b.startedAt
            }
    }

    private func send(_ obj: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: obj),
              let text = String(data: data, encoding: .utf8) else { return }
        task?.send(.string(text)) { [weak self] err in
            if let err { Task { @MainActor in self?.lastError = err.localizedDescription } }
        }
    }

    private func receiveLoop() {
        guard receiveActive, let task else { return }
        task.receive { [weak self] result in
            Task { @MainActor in
                guard let self, self.receiveActive else { return }
                switch result {
                case .success(let msg):
                    if case .string(let text) = msg, let data = text.data(using: .utf8) {
                        self.handleFrame(data)
                    }
                    self.receiveLoop()
                case .failure(let err):
                    self.connected = false
                    self.lastError = err.localizedDescription
                }
            }
        }
    }

    private func handleFrame(_ data: Data) {
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let type = json["type"] as? String else { return }
        switch type {
        case "hello":
            if let raw = json["sessions"] as? [[String: Any]] {
                sessions = decodeSessions(raw)
            }
            if let paths = json["cwds"] as? [String] { cwds = paths }
        case "sessions":
            if let raw = json["sessions"] as? [[String: Any]] {
                sessions = decodeSessions(raw)
            }
        case "cwds":
            if let paths = json["cwds"] as? [String] { cwds = paths }
        case "event":
            if let evt = json["event"] as? [String: Any] {
                handleEvent(evt)
            }
        default:
            break
        }
    }

    private func handleEvent(_ evt: [String: Any]) {
        guard (evt["type"] as? String) == "system",
              let sub = evt["subtype"] as? String else { return }
        switch sub {
        case "session_created", "session_updated":
            if let raw = evt["session"] as? [String: Any],
               let s = decodeSession(raw) {
                if let i = sessions.firstIndex(where: { $0.id == s.id }) {
                    sessions[i] = s
                } else {
                    sessions.append(s)
                }
            }
        case "session_deleted":
            if let id = evt["sessionId"] as? String {
                sessions.removeAll { $0.id == id }
            }
        default:
            break
        }
    }

    private func decodeSessions(_ raw: [[String: Any]]) -> [SynapseSession] {
        raw.compactMap { decodeSession($0) }
    }

    private func decodeSession(_ dict: [String: Any]) -> SynapseSession? {
        guard let data = try? JSONSerialization.data(withJSONObject: dict) else { return nil }
        return try? JSONDecoder().decode(SynapseSession.self, from: data)
    }

    private func normalizePath(_ p: String) -> String {
        var s = p.trimmingCharacters(in: .whitespacesAndNewlines)
        while s.count > 1 && s.hasSuffix("/") { s.removeLast() }
        return s
    }
}
