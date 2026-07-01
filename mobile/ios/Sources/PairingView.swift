import SwiftUI

struct PairingView: View {
    @EnvironmentObject private var app: AppModel
    @State private var host = "127.0.0.1"
    @State private var port = "4173"
    @State private var token = "CODE"
    @State private var tls = false

    var body: some View {
        NavigationStack {
            Form {
                Section("Server") {
                    TextField("Host", text: $host)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    TextField("Port", text: $port)
                        .keyboardType(.numberPad)
                    SecureField("Token", text: $token)
                    Toggle("Secure (wss)", isOn: $tls)
                }
                Section {
                    Button("Connect") {
                        app.savePairing(host: host, port: port, token: token, tls: tls)
                    }
                    .frame(maxWidth: .infinity, alignment: .center)
                }
            }
            .navigationTitle("Connect")
        }
    }
}
