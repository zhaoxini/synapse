import SwiftUI
import WebKit
import ObjectiveC

/// Full-screen host for the Figma-aligned web UI (`crates/app/web`). Same bundle as browser :8000.
@MainActor
final class WebShell: NSObject, ObservableObject, WKNavigationDelegate {
    @Published private(set) var webView: WKWebView?

    func boot() {
        let creds = SynapseCreds.load() ?? SynapseCreds.simulatorDefault
        setenv("SYNAPSE_HOST", creds.host, 1)
        setenv("SYNAPSE_PORT", creds.port, 1)
        setenv("SYNAPSE_TOKEN", creds.token, 1)
        setenv("SYNAPSE_TLS", creds.tls ? "1" : "0", 1)

        guard let cstr = synapse_web_url() else { return }
        defer { free(cstr) }
        guard let url = URL(string: String(cString: cstr)) else { return }

        if webView == nil {
            webView = makeWebView()
        }
        webView?.load(URLRequest(url: url))
    }

    private func makeWebView() -> WKWebView {
        let cfg = WKWebViewConfiguration()
        cfg.allowsInlineMediaPlayback = true
        let handler = WebShellScriptHandler(owner: self)
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
        wv.navigationDelegate = self
        wv.scrollView.contentInsetAdjustmentBehavior = .never
        wv.isOpaque = false
        wv.backgroundColor = .systemBackground
        return wv
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        Self.removeInputAccessory(from: webView)
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
        case "inputFocus":
            if let wv = webView { Self.removeInputAccessory(from: wv) }
        default:
            break
        }
    }

    private static func removeInputAccessory(from webView: WKWebView) {
        guard let contentView = webView.scrollView.subviews.first(where: {
            String(describing: type(of: $0)).hasPrefix("WKContent")
        }) else { return }
        let subName = String(describing: type(of: contentView)) + "_NoAccessory"
        if let subclass = NSClassFromString(subName) {
            object_setClass(contentView, subclass)
            return
        }
        guard let subclass = objc_allocateClassPair(object_getClass(contentView), subName, 0) else { return }
        let nilImp = imp_implementationWithBlock({ (_: Any?) -> Any? in nil } as @convention(block) (Any?) -> Any?)
        class_addMethod(subclass, #selector(getter: UIResponder.inputAccessoryView), nilImp, "@@:")
        objc_registerClassPair(subclass)
        object_setClass(contentView, subclass)
    }
}

private final class WebShellScriptHandler: NSObject, WKScriptMessageHandler {
    weak var owner: WebShell?
    init(owner: WebShell) { self.owner = owner }
    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard message.name == "synapse",
              let body = message.body as? [String: Any] else { return }
        Task { @MainActor in owner?.handleMessage(body) }
    }
}

struct WebShellView: UIViewRepresentable {
    let webView: WKWebView

    func makeUIView(context: Context) -> WKWebView { webView }
    func updateUIView(_ uiView: WKWebView, context: Context) {}
}
