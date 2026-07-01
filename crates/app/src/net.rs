//! Background network thread for the Synapse app.
//!
//! The main thread runs only the Slint event loop (`app.run()`) and never
//! touches tokio I/O. All WebSocket work happens on a dedicated background
//! thread that owns its own multi-thread tokio runtime.
//!
//! Bridge:
//!   UI → net:  UI callbacks push [`NetCmd`] onto an `mpsc` channel.
//!   net → UI:  the net thread calls `slint::invoke_from_event_loop` with a
//!              `Weak<App>` to mutate UI state on the main thread.

use futures_util::{SinkExt, StreamExt};
use slint::Weak;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

use crate::{handle_event, App};

/// A parsed `synapse://host:port?token=T&tls=N` pairing link.
pub struct ParsedPair {
    pub host: String,
    pub port: String,
    pub token: String,
    pub tls: bool,
    /// URL path for relay connections (e.g. "/connect"). Empty for direct
    /// server connections (which use "/").
    pub path: String,
    /// Relay device id (required for /connect).
    pub device_id: String,
}

/// Commands the UI sends to the background network thread.
pub enum NetCmd {
    /// Establish a WebSocket connection with these credentials (drops any
    /// existing connection and starts a fresh connect/pump/reconnect loop).
    Connect {
        host: String,
        port: String,
        token: String,
        tls: bool,
        path: String,
        device_id: String,
    },
    /// Send a raw JSON text frame over the active connection. Silently
    /// dropped if there is no active connection.
    Send(String),
}

/// Cheap handle the UI thread uses to push commands to the net thread.
#[derive(Clone)]
pub struct NetHandle {
    tx: Sender<NetCmd>,
}

impl NetHandle {
    pub fn send(&self, cmd: NetCmd) {
        let _ = self.tx.send(cmd);
    }
}

/// Spawn the background network thread. Returns a handle the UI uses to push
/// commands. The thread owns a private multi-thread tokio runtime so its I/O
/// never shares context with the Slint event loop (which on iOS runs outside
/// any tokio reactor).
pub fn spawn_net_thread(weak: Weak<App>) -> NetHandle {
    let (tx, rx) = mpsc::channel::<NetCmd>();
    thread::Builder::new()
        .name("synapse-net".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("create net tokio runtime");
            rt.block_on(net_main(weak, rx));
        })
        .expect("spawn net thread");
    NetHandle { tx }
}

/// Main loop of the background network thread. It owns the command receiver
/// (single consumer) and, when a `Connect` arrives, spawns a connection task
/// and keeps a sender to forward subsequent `Send` commands into it.
async fn net_main(weak: Weak<App>, rx: Receiver<NetCmd>) {
    // Sender into the currently-active connection task (if any).
    let mut active_send: Option<tokio::sync::mpsc::UnboundedSender<String>> = None;
    let mut active_task: Option<tokio::task::JoinHandle<()>> = None;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            NetCmd::Connect {
                host,
                port,
                token,
                tls,
                path,
                device_id,
            } => {
                // Abort the previous connection task and start fresh.
                if let Some(t) = active_task.take() {
                    t.abort();
                }
                let (send_tx, send_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                active_send = Some(send_tx);
                let url = build_url(&host, &port, &token, tls, &path, &device_id);
                let weak = weak.clone();
                active_task = Some(tokio::spawn(async move {
                    run_connection(&url, tls, &weak, send_rx).await;
                }));
            }
            NetCmd::Send(text) => {
                if let Some(tx) = &active_send {
                    let _ = tx.send(text);
                }
            }
        }
    }
    // Channel closed (UI thread gone) — let the runtime drop.
}

fn build_url(host: &str, port: &str, token: &str, tls: bool, path: &str, device_id: &str) -> String {
    let scheme = if tls { "wss" } else { "ws" };
    if path.is_empty() {
        format!("{scheme}://{host}:{port}/?token={token}")
    } else if device_id.is_empty() {
        format!("{scheme}://{host}:{port}{path}?token={token}")
    } else {
        format!("{scheme}://{host}:{port}{path}?deviceId={device_id}&token={token}")
    }
}

/// Connect, pump events to the UI, and reconnect with capped exponential
/// backoff until the app exits or a new `Connect` supersedes this task.
async fn run_connection(
    url: &str,
    tls: bool,
    weak: &Weak<App>,
    mut send_rx: tokio::sync::mpsc::UnboundedReceiver<String>,
) {
    let mut backoff = Duration::from_secs(1);
    let mut first = true;

    loop {
        let stream = match connect_once(url, tls).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[synapse-net] connect failed: {e}");
                if first {
                    // Surface the failure on the pairing screen.
                    let msg = format!("Could not connect: {e}");
                    invoke(weak, move |app| {
                        app.set_connecting(false);
                        app.set_pairingError(msg.into());
                    });
                    return;
                }
                sleep_backoff(&mut backoff).await;
                continue;
            }
        };

        let (mut tx, mut rx) = stream.split();

        // Prime: request the session list right away.
        let _ = tx
            .send(Message::Text(serde_json::json!({"op":"list"}).to_string()))
            .await;

        if first {
            first = false;
            invoke(weak, |app| {
                app.set_connecting(false);
                app.set_view("chat".into());
                app.set_drawerOpen(true);
            });
        } else {
            // Re-request the active session's transcript on reconnect. We need
            // the activeSessionId from the UI thread, so fetch it via a
            // one-shot sync channel inside invoke_from_event_loop.
            let (sid_tx, sid_rx) = std::sync::mpsc::channel::<String>();
            invoke_with_ret(weak, sid_tx, |app| app.get_activeSessionId().to_string());
            invoke(weak, |app| {
                app.set_reconnecting(false);
                app.set_showToast(false);
            });
            if let Ok(sid) = sid_rx.recv() {
                if !sid.is_empty() {
                    let msg = serde_json::json!(
                        { "op": "history", "sessionId": sid, "limit": 400 }
                    )
                    .to_string();
                    let _ = tx.send(Message::Text(msg)).await;
                }
            }
        }

        backoff = Duration::from_secs(1);

        // Pump: interleave inbound socket reads with outbound sends.
        loop {
            tokio::select! {
                msg = rx.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            let v: serde_json::Value = match serde_json::from_str(&text) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };
                            let weak = weak.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(app) = weak.upgrade() {
                                    handle_event(&app, v);
                                }
                            });
                        }
                        Some(Ok(_)) => continue,
                        _ => break, // stream ended or errored
                    }
                }
                out = send_rx.recv() => {
                    match out {
                        Some(text) => {
                            let _ = tx.send(Message::Text(text)).await;
                        }
                        None => return, // net_main dropped the sender → superseded/closing
                    }
                }
            }
        }

        // Stream ended — surface a toast and retry.
        invoke(weak, |app| {
            app.set_toast("Connection lost — retrying…".into());
            app.set_showToast(true);
            app.set_reconnecting(true);
        });
        // Drain any queued sends while disconnected.
        while send_rx.try_recv().is_ok() {}

        sleep_backoff(&mut backoff).await;
    }
}

async fn connect_once(
    url: &str,
    tls: bool,
) -> anyhow::Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    if tls {
        make_tls_stream(url).await
    } else {
        let (s, _) = tokio_tungstenite::connect_async(url).await?;
        Ok(s)
    }
}

async fn sleep_backoff(backoff: &mut Duration) {
    let d = *backoff;
    *backoff = (*backoff * 2).min(Duration::from_secs(15));
    tokio::time::sleep(d).await;
}

/// Like [`invoke`] but also passes a single value back from the UI thread via
/// a sync channel. Used when the net thread needs to read a UI property.
fn invoke_with_ret<T: Send + 'static>(
    weak: &Weak<App>,
    ret_tx: std::sync::mpsc::Sender<T>,
    f: impl FnOnce(&App) -> T + Send + 'static,
) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            let _ = ret_tx.send(f(&app));
        }
    });
}

/// Invoke a closure on the main (Slint) thread.
fn invoke(weak: &Weak<App>, f: impl FnOnce(&App) + Send + 'static) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            f(&app);
        }
    });
}

// --- TLS stream with permissive (self-signed) cert acceptance ---
async fn make_tls_stream(
    url: &str,
) -> anyhow::Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let request = url.into_client_request()?;

    let config = rustls::client::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(std::sync::Arc::new(AcceptAnyCert))
        .with_no_client_auth();
    let connector = tokio_tungstenite::Connector::Rustls(std::sync::Arc::new(config));

    let (stream, _resp) =
        tokio_tungstenite::connect_async_tls_with_config(request, None, false, Some(connector))
            .await?;
    Ok(stream)
}

#[derive(Debug)]
struct AcceptAnyCert;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls_pki_types::CertificateDer<'_>,
        _intermediates: &[rustls_pki_types::CertificateDer<'_>],
        _server_name: &rustls_pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls_pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls_pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls_pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

/// Parse a pairing link emitted by the server's QR code. Accepts
/// `synapse://host:port?token=...&tls=0|1` (also tolerates a bare
/// `host:port?token=...` and a `wss?://` scheme for convenience).
pub fn parse_pair_link(link: &str) -> Option<ParsedPair> {
    let raw = link.trim();
    if raw.is_empty() {
        return None;
    }
    // Accept the synapse:// scheme or a bare authority. Also tolerate
    // wss:// / ws:// for convenience (e.g. pasted from a browser).
    let body = raw
        .strip_prefix("synapse://")
        .or_else(|| raw.strip_prefix("synapse:"))
        .or_else(|| raw.strip_prefix("wss://"))
        .or_else(|| raw.strip_prefix("ws://"))
        .unwrap_or(raw);
    // Split authority+path from query.
    let (auth_path, query) = body.split_once('?').unwrap_or((body, ""));

    // Parse query params token / tls / deviceId first so we can detect a
    // relay link (which carries deviceId and embeds a /connect path).
    let mut token = String::new();
    let mut tls = false;
    let mut device_id = String::new();
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            match k {
                "token" => token = v.to_string(),
                "tls" => tls = v == "1" || v.eq_ignore_ascii_case("true"),
                "deviceId" => device_id = v.to_string(),
                _ => {}
            }
        }
    }
    if token.is_empty() {
        return None;
    }

    // Relay link shape: synapse://relay.example.com/connect?deviceId=...&token=...
    // The authority portion contains a path suffix ("/connect"). Split it off.
    if let Some(slash) = auth_path.find('/') {
        let (authority, path) = auth_path.split_at(slash);
        let (host, port) = split_host_port(authority, tls);
        if host.is_empty() {
            return None;
        }
        return Some(ParsedPair {
            host,
            port,
            token,
            tls,
            path: path.to_string(),
            device_id,
        });
    }

    // Standard direct link: synapse://host:port?token=...&tls=...
    let (host, port) = split_host_port(auth_path, tls);
    if host.is_empty() {
        return None;
    }
    Some(ParsedPair {
        host,
        port,
        token,
        tls,
        path: String::new(),
        device_id,
    })
}

/// Split "host" or "host:port" into (host, port). When no port is present,
/// default to 443 for TLS links and 80 for plain links (covers relay links
/// which omit the port because the relay runs on standard wss/http ports).
fn split_host_port(authority: &str, tls: bool) -> (String, String) {
    match authority.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            (h.to_string(), p.to_string())
        }
        _ => (
            authority.to_string(),
            if tls {
                "443".to_string()
            } else {
                "80".to_string()
            },
        ),
    }
}
