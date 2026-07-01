import Foundation

enum WebHost {
    /// Embedded localhost URL for the bundled web chat (append query params as needed).
    static func chatBaseURL(creds: SynapseCreds) -> URL? {
        setenv("SYNAPSE_HOST", creds.host, 1)
        setenv("SYNAPSE_PORT", creds.port, 1)
        setenv("SYNAPSE_TOKEN", creds.token, 1)
        setenv("SYNAPSE_TLS", creds.tls ? "1" : "0", 1)

        guard let cstr = synapse_web_url() else { return nil }
        defer { free(cstr) }
        guard var url = URL(string: String(c: cstr)) else { return nil }

        var comp = URLComponents(url: url, resolvingAgainstBaseURL: false)
        var items = comp?.queryItems ?? []
        items.append(URLQueryItem(name: "shell", value: "native"))
        comp?.queryItems = items
        return comp?.url ?? url
    }
}
