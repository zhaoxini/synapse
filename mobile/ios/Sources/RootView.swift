import SwiftUI

struct ChatScreen: View {
    @EnvironmentObject private var app: AppModel
    let session: SynapseSession

    var body: some View {
        Group {
            if let wv = app.chatHost.webView {
                ChatWebView(webView: wv)
                    .ignoresSafeArea(edges: .bottom)
            } else {
                ProgressView("Loading chat…")
            }
        }
        .navigationTitle(session.displayTitle)
        .navigationBarTitleDisplayMode(.inline)
        .navigationBarBackButtonHidden(true)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button {
                    app.closeChat()
                } label: {
                    HStack(spacing: 4) {
                        Image(systemName: "chevron.left")
                        Text("Workspaces")
                    }
                }
            }
        }
        .onAppear {
            app.chatHost.openSession(session.id)
        }
    }
}

struct RootView: View {
    @EnvironmentObject private var app: AppModel

    var body: some View {
        NavigationStack {
            WorkspacesView()
                .navigationDestination(item: $app.activeSession) { session in
                    ChatScreen(session: session)
                }
        }
        .onAppear { app.boot() }
    }
}
