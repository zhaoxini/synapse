import SwiftUI
import WebKit

/// Hosts the chat-only WKWebView and bridges to window.__synapse.openSession.
@MainActor
final class ChatWebHost: ObservableObject {
    @Published private(set) var webView: WKWebView?
    private var baseURL: URL?
    private var pendingSessionId: String?

    func prepare(creds: SynapseCreds) {
        guard let url = WebHost.chatBaseURL(creds: creds) else { return }
        baseURL = url
        if webView == nil {
            webView = makeWebView()
        }
        webView?.load(URLRequest(url: url))
    }

    func reset() {
        webView?.loadHTMLString("", baseURL: nil)
        webView = nil
        baseURL = nil
        pendingSessionId = nil
    }

    func markReady() {
        if let id = pendingSessionId {
            openSession(id)
            pendingSessionId = nil
        }
    }

    func openSession(_ id: String) {
        guard let webView else {
            pendingSessionId = id
            return
        }
        let js = "window.__synapse && window.__synapse.openSession('\(id.replacingOccurrences(of: "'", with: "\\'"))');"
        webView.evaluateJavaScript(js, completionHandler: nil)
    }

    private func makeWebView() -> WKWebView {
        let cfg = WKWebViewConfiguration()
        cfg.allowsInlineMediaPlayback = true
        let handler = ChatScriptHandler(owner: self)
        cfg.userContentController.add(handler, name: "synapse")
        let bridge = """
        window.__synapseHaptic__=function(s){try{webkit.messageHandlers.synapse.postMessage({op:'haptic',style:s||'light'});}catch(e){}};
        window.__synapseCopy__=function(t){try{webkit.messageHandlers.synapse.postMessage({op:'copy',text:String(t||'')});}catch(e){}};
        document.addEventListener('focusin',function(e){if(e.target&&e.target.id==='input'){try{webkit.messageHandlers.synapse.postMessage({op:'inputFocus'});}catch(x){}}},true);
        """
        cfg.userContentController.addUserScript(
            WKUserScript(source: bridge, injectionTime: .atDocumentStart, forMainFrameOnly: true)
        )
        let wv = WKWebView(frame: .zero, configuration: cfg)
        wv.scrollView.contentInsetAdjustmentBehavior = .never
        wv.isOpaque = false
        wv.backgroundColor = .systemBackground
        return wv
    }

    fileprivate func handleMessage(_ body: [String: Any]) {
        guard let op = body["op"] as? String else { return }
        switch op {
        case "haptic":
            let style = body["style"] as? String ?? "light"
            let gen = UIImpactFeedbackGenerator(style: style == "heavy" ? .heavy : style == "medium" ? .medium : .light)
            gen.prepare()
            gen.impactOccurred()
        case "copy":
            if let text = body["text"] as? String { UIPasteboard.general.string = text }
        case "chatReady":
            markReady()
        default:
            break
        }
    }
}

private final class ChatScriptHandler: NSObject, WKScriptMessageHandler {
    weak var owner: ChatWebHost?
    init(owner: ChatWebHost) { self.owner = owner }
    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard message.name == "synapse",
              let body = message.body as? [String: Any] else { return }
        Task { @MainActor in owner?.handleMessage(body) }
    }
}

struct ChatWebView: UIViewRepresentable {
    let webView: WKWebView

    func makeUIView(context: Context) -> WKWebView { webView }
    func updateUIView(_ uiView: WKWebView, context: Context) {}
}
