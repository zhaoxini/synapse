import Foundation

struct SynapseCreds: Codable, Equatable {
    var host: String
    var port: String
    var token: String
    var tls: Bool
    var path: String

    static let storageKey = "synapse_creds"

    var wsURL: URL? {
        let scheme = tls ? "wss" : "ws"
        var comp = URLComponents()
        comp.scheme = scheme
        comp.host = host
        comp.port = Int(port)
        comp.path = path.isEmpty ? "/" : path
        comp.queryItems = [URLQueryItem(name: "token", value: token)]
        return comp.url
    }

    static func load() -> SynapseCreds? {
        guard let data = UserDefaults.standard.data(forKey: storageKey),
              let c = try? JSONDecoder().decode(SynapseCreds.self, from: data) else { return nil }
        return c
    }

    func save() {
        if let data = try? JSONEncoder().encode(self) {
            UserDefaults.standard.set(data, forKey: Self.storageKey)
        }
    }

    /// Sim / dev defaults (matches legacy main.m).
    static var simulatorDefault: SynapseCreds {
        SynapseCreds(host: "127.0.0.1", port: "4173", token: "CODE", tls: false, path: "")
    }
}

struct SynapseSession: Identifiable, Equatable, Hashable, Codable {
    let id: String
    var name: String?
    var cwd: String
    var state: String
    var startedAt: UInt64
    var pinned: Bool
    var archived: Bool
    var diffAdds: Int
    var diffDels: Int

    enum CodingKeys: String, CodingKey {
        case id, name, cwd, state, pinned, archived
        case startedAt = "started_at"
        case diffAdds = "diff_adds"
        case diffDels = "diff_dels"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decodeIfPresent(String.self, forKey: .name)
        cwd = try c.decodeIfPresent(String.self, forKey: .cwd) ?? ""
        state = try c.decodeIfPresent(String.self, forKey: .state) ?? "idle"
        startedAt = try c.decodeIfPresent(UInt64.self, forKey: .startedAt) ?? 0
        pinned = try c.decodeIfPresent(Bool.self, forKey: .pinned) ?? false
        archived = try c.decodeIfPresent(Bool.self, forKey: .archived) ?? false
        diffAdds = try c.decodeIfPresent(Int.self, forKey: .diffAdds) ?? 0
        diffDels = try c.decodeIfPresent(Int.self, forKey: .diffDels) ?? 0
    }

    var displayTitle: String {
        let raw = (name ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if !raw.isEmpty { return raw }
        return "New session"
    }

    var workspaceLabel: String {
        URL(fileURLWithPath: cwd).lastPathComponent
    }

    func hash(into hasher: inout Hasher) { hasher.combine(id) }
}

struct WorkspaceRow: Identifiable, Equatable {
    var path: String
    var id: String { path }
    var label: String { URL(fileURLWithPath: path).lastPathComponent }
}
