// Synapse relay — account service + WebSocket bridge for remote access.
//
//   mobile app  --wss-->  relay  <--wss--  synapse-server (outbound only)
//
// The relay stores users/devices in SQLite, exposes a REST API for login and
// device discovery, and shuttles WS frames transparently.

mod api;
mod auth;
mod db;
mod registry;

use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Query, State,
    },
    response::IntoResponse,
    routing::get,
};
use clap::Parser;
use db::Db;
use futures_util::{SinkExt, StreamExt};
use registry::Registry;
use serde::Deserialize;
use serde_json::json;
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[command(
    name = "synapse-relay",
    version,
    about = "Synapse relay: accounts, device registry, and WebSocket bridge"
)]
struct Args {
    #[arg(short, long, default_value = "443")]
    port: u16,
    #[arg(long, default_value = "0.0.0.0")]
    host: String,
    /// Public hostname shown to clients (for pairing URLs). Defaults to --host.
    #[arg(long)]
    public_host: Option<String>,
    /// Public port shown to clients (when behind a reverse proxy). Defaults to --port.
    #[arg(long)]
    public_port: Option<u16>,
    /// Public TLS flag shown to clients (when TLS terminates at a reverse proxy).
    #[arg(long, default_value_t = false)]
    public_tls: bool,
    #[arg(long)]
    tls_cert: Option<PathBuf>,
    #[arg(long)]
    tls_key: Option<PathBuf>,
    /// SQLite database path.
    #[arg(long, default_value = "synapse-relay.db")]
    db: PathBuf,
    #[arg(long)]
    dev: bool,
}

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
    pub registry: Arc<Registry>,
    pub public_host: String,
    pub public_port: u16,
    pub tls: bool,
}

#[derive(Deserialize)]
struct DeviceQ {
    #[serde(default, alias = "deviceId")]
    device_id: Option<String>,
    #[serde(default)]
    token: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "synapse_relay=info".into()),
        )
        .init();

    let args = Args::parse();
    let db = Arc::new(Db::open(&args.db)?);
    let registry = Arc::new(Registry::new());
    let tls = args.tls_cert.is_some();
    let public_host = args
        .public_host
        .clone()
        .unwrap_or_else(|| args.host.clone());
    let public_port = args.public_port.unwrap_or(args.port);
    let public_tls = args.public_tls || tls;

    let ws_state = AppState {
        db: db.clone(),
        registry: registry.clone(),
        public_host: public_host.clone(),
        public_port,
        tls: public_tls,
    };

    let app = api::router()
        .route("/uplink", get(uplink))
        .route("/connect", get(connect))
        .with_state(ws_state);

    let addr: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;
    let scheme = if tls { "wss" } else { "ws" };

    println!("\n  Synapse relay is running.\n");
    println!("  Listen:        {scheme}://{addr}");
    println!("  Public host:   {public_host}");
    println!("  Database:      {}", args.db.display());
    println!(
        "  API:           {scheme}://{public_host}:{}/api/v1/...",
        args.port
    );
    println!("  Server uplink: {scheme}://{public_host}/uplink?deviceId=<ID>&token=<DEVICE_TOKEN>");
    println!(
        "  App connect:   {scheme}://{public_host}/connect?deviceId=<ID>&token=<CONNECT_TOKEN>\n"
    );

    if let (Some(cert), Some(key)) = (args.tls_cert.as_ref(), args.tls_key.as_ref()) {
        let cfg = load_tls(cert, key).await?;
        axum_server::bind_rustls(addr, cfg)
            .serve(app.into_make_service())
            .await?;
    } else {
        tracing::warn!(
            "TLS disabled — local testing only. Public deployments need --tls-cert/--tls-key."
        );
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app.into_make_service()).await?;
    }
    Ok(())
}

async fn load_tls(
    cert: &std::path::Path,
    key: &std::path::Path,
) -> Result<axum_server::tls_rustls::RustlsConfig> {
    let cert_pem = std::fs::read(cert).context("read --tls-cert")?;
    let key_pem = std::fs::read(key).context("read --tls-key")?;
    Ok(axum_server::tls_rustls::RustlsConfig::from_pem(cert_pem, key_pem).await?)
}

async fn uplink(
    State(s): State<AppState>,
    Query(q): Query<DeviceQ>,
    ws: axum::extract::WebSocketUpgrade,
) -> impl IntoResponse {
    let device_id = match q.device_id.clone() {
        Some(d) if !d.is_empty() => d,
        _ => return axum::http::StatusCode::BAD_REQUEST.into_response(),
    };
    let token = match q.token.as_deref() {
        Some(t) if !t.is_empty() => t,
        _ => return axum::http::StatusCode::UNAUTHORIZED.into_response(),
    };
    match api::verify_uplink_token(&s.db, &device_id, token) {
        Ok(true) => {}
        Ok(false) => return axum::http::StatusCode::UNAUTHORIZED.into_response(),
        Err(e) => {
            tracing::error!("uplink auth db error: {e}");
            return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    ws.on_upgrade(move |socket| uplink_loop(s, device_id, socket))
}

async fn uplink_loop(state: AppState, device_id: String, socket: WebSocket) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (server_tx, mut server_rx) = mpsc::channel::<Message>(128);

    let app_slot = state.registry.register(&device_id, server_tx.clone()).await;
    tracing::info!(%device_id, "uplink registered");

    let to_server = tokio::spawn(async move {
        while let Some(m) = server_rx.recv().await {
            if ws_tx.send(m).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Ping(p) => {
                let _ = server_tx.send(Message::Pong(p)).await;
            }
            Message::Close(_) => break,
            other => {
                if let Some(app_tx) = app_slot.lock().await.as_ref() {
                    let _ = app_tx.send(other).await;
                }
            }
        }
    }

    state.registry.unregister(&device_id).await;
    tracing::info!(%device_id, "uplink gone");
    to_server.abort();
}

async fn connect(
    State(s): State<AppState>,
    Query(q): Query<DeviceQ>,
    ws: axum::extract::WebSocketUpgrade,
) -> impl IntoResponse {
    let device_id = match q.device_id.clone() {
        Some(d) if !d.is_empty() => d,
        _ => return axum::http::StatusCode::BAD_REQUEST.into_response(),
    };
    let token = match q.token.as_deref() {
        Some(t) if !t.is_empty() => t,
        _ => return axum::http::StatusCode::UNAUTHORIZED.into_response(),
    };
    match api::verify_ws_token(&s.db, &device_id, token) {
        Ok(true) => {}
        Ok(false) => return axum::http::StatusCode::UNAUTHORIZED.into_response(),
        Err(e) => {
            tracing::error!("connect auth db error: {e}");
            return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    if !s.registry.is_online(&device_id).await {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    ws.on_upgrade(move |socket| connect_loop(s, device_id, socket))
}

async fn connect_loop(state: AppState, device_id: String, socket: WebSocket) {
    let (server_tx, app_slot) = match state.registry.acquire(&device_id).await {
        Some(v) => v,
        None => {
            let (mut tx, _rx) = socket.split();
            let _ = tx
                .send(Message::Text(
                    json!({"type":"error","error":"device offline"}).to_string(),
                ))
                .await;
            return;
        }
    };

    let (mut app_tx, mut app_rx) = socket.split();
    let (to_app_tx, mut to_app_rx) = mpsc::channel::<Message>(128);

    {
        let mut slot = app_slot.lock().await;
        *slot = Some(to_app_tx.clone());
    }
    tracing::info!(%device_id, "app linked");

    let s1 = tokio::spawn(async move {
        while let Some(m) = to_app_rx.recv().await {
            if app_tx.send(m).await.is_err() {
                break;
            }
        }
    });

    let slot = app_slot.clone();
    let s2 = tokio::spawn(async move {
        while let Some(Ok(msg)) = app_rx.next().await {
            if let Message::Close(_) = msg {
                break;
            }
            if server_tx.send(msg).await.is_err() {
                break;
            }
        }
        let mut s = slot.lock().await;
        *s = None;
    });

    let _ = (s1.await, s2.await);
    tracing::info!(%device_id, "app unlinked");
}
