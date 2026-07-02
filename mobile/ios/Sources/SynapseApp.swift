import SwiftUI

@main
struct SynapseApp: App {
    @StateObject private var shell = WebShell()

    var body: some Scene {
        WindowGroup {
            Group {
                if let wv = shell.webView {
                    WebShellView(webView: wv)
                        .ignoresSafeArea()
                } else {
                    ProgressView("Loading…")
                }
            }
            .onAppear { shell.boot() }
        }
    }
}
