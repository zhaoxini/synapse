mod account;
mod claude;
mod history;
mod http;
mod manager;
mod models;
mod relay;
mod startup;
mod tail;
mod tls;
mod tunnel;
mod web_ui;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use manager::SessionManager;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::net::TcpListener;

/// Synapse server — remote mobile control for the Claude Code CLI.
#[derive(Parser, Debug)]
#[command(name = "synapse-server", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    #[command(flatten)]
    run: RunArgs,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the local bridge server (default when no subcommand is given).
    Run,
    /// Create an account and register this machine with the Synapse relay.
    Register {
        /// Relay URL, e.g. wss://relay.example.com or https://relay.example.com
        #[arg(long)]
        relay: String,
        #[arg(long)]
        email: String,
        #[arg(long)]
        password: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        device_name: Option<String>,
    },
    /// Log in and register this machine with the Synapse relay.
    Login {
        #[arg(long)]
        relay: String,
        #[arg(long)]
        email: String,
        #[arg(long)]
        password: String,
        #[arg(long)]
        device_name: Option<String>,
    },
    /// Print a short pairing code for the mobile app (requires prior login).
    PairingCode,
    /// Show saved account / device info.
    Status,
    /// Sign out — remove local credentials (does not delete the cloud account).
    Logout,
}

/// Arguments for `synapse-server run` (also the default when no subcommand).
#[derive(Parser, Debug)]
struct RunArgs {
    #[arg(short, long, default_value = "4173")]
    port: u16,
    #[arg(long, default_value = "0.0.0.0")]
    host: String,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long)]
    token: Option<String>,
    #[arg(long)]
    bin: Option<PathBuf>,
    #[arg(long)]
    tls: bool,
    #[arg(long)]
    tls_cert: Option<PathBuf>,
    #[arg(long)]
    tls_key: Option<PathBuf>,
    #[arg(long)]
    tls_self_signed: bool,
    #[arg(long)]
    tls_san: Option<String>,
    #[arg(long)]
    tls_cert_out: Option<PathBuf>,
    #[arg(long)]
    tls_key_out: Option<PathBuf>,
    #[arg(long)]
    pair_host: Option<String>,
    #[arg(long)]
    tunnel: bool,
    #[arg(long)]
    relay: Option<String>,
    #[arg(long)]
    relay_device_id: Option<String>,
    #[arg(long)]
    relay_token: Option<String>,
    #[arg(long)]
    dev: bool,
    #[arg(long)]
    default_model: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let log_dir = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".synapse")
        .join("logs");
    std::fs::create_dir_all(&log_dir).ok();
    let file_appender = tracing_appender::rolling::daily(&log_dir, "server.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "synapse_server=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        None | Some(Commands::Run) => {
            if account::Config::load()?.is_none() {
                account::interactive_setup().await?;
            }
            run_server(cli.run).await
        }
        Some(Commands::Register {
            relay,
            email,
            password,
            name,
            device_name,
        }) => {
            let device_name = device_name.unwrap_or_else(account::default_device_name);
            let name = name.unwrap_or_default();
            let cfg =
                account::register_account(&relay, &email, &password, &name, &device_name).await?;
            cfg.save()?;
            println!("\n  Account registered and device linked.\n");
            println!("  Email:       {}", cfg.user_email);
            println!("  Device:      {} ({})", cfg.device_name, cfg.device_id);
            println!("  Config:      {}", account::Config::path().display());
            println!("\n  Run: synapse-server\n");
            Ok(())
        }
        Some(Commands::Login {
            relay,
            email,
            password,
            device_name,
        }) => {
            let device_name = device_name.unwrap_or_else(account::default_device_name);
            let cfg = account::login_account(&relay, &email, &password, &device_name).await?;
            cfg.save()?;
            println!("\n  Logged in and device registered.\n");
            println!("  Email:       {}", cfg.user_email);
            println!("  Device:      {} ({})", cfg.device_name, cfg.device_id);
            println!("  Config:      {}", account::Config::path().display());
            println!("\n  Run: synapse-server\n");
            Ok(())
        }
        Some(Commands::PairingCode) => {
            let cfg =
                account::Config::load()?.context("not signed in — run synapse-server first")?;
            if let Some(code) = account::load_pairing_code() {
                println!("\n  Pairing code:  {}\n", code);
                println!("  Valid while synapse-server is running.\n");
                println!("  Web: http://127.0.0.1:8000/?code={code}\n",);
                return Ok(());
            }
            let code = account::create_pairing_code(&cfg).await?;
            println!("\n  Pairing code:  {}\n", code.code);
            println!("  Valid while synapse-server is running.\n");
            println!(
                "  Enter this code in the Synapse app (same account: {}).\n",
                cfg.user_email
            );
            Ok(())
        }
        Some(Commands::Status) => {
            let cfg =
                account::Config::load()?.context("not signed in — run synapse-server first")?;
            account::print_status(&cfg);
            Ok(())
        }
        Some(Commands::Logout) => {
            account::clear_config()?;
            println!("\n  Signed out. Local credentials removed.\n");
            println!("  Run synapse-server again to sign in.\n");
            Ok(())
        }
    }
}

async fn run_server(args: RunArgs) -> Result<()> {
    let saved = account::Config::load()?;
    let cwd = args
        .cwd
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let bin = claude::ClaudeBin::resolve(args.bin.as_ref());

    let default_model = args
        .default_model
        .or_else(|| std::env::var("SYNAPSE_DEFAULT_MODEL").ok());
    let manager = SessionManager::new(bin.clone(), cwd.clone(), default_model);
    let addr: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;
    let (router, token) = http::router(manager.clone(), args.token, addr.port());
    let scheme = if args.tls { "wss" } else { "ws" };

    println!("\n  Synapse server is running.\n");
    println!("  Claude binary:  {}", bin.0.display());
    println!("  Working dir:    {}", cwd.display());

    if saved.is_some() {
        web_ui::spawn();
        // Account mode — phone connects via relay; keep output minimal.
        if let Some(cfg) = &saved {
            println!("  Signed in as:   {}", cfg.user_email);
            println!("  This machine:   {}", cfg.device_name);
            if let Ok(code) = account::create_pairing_code(cfg).await {
                println!("\n  ┌─────────────────────────────────────┐");
                println!("  │  Pairing code:  {:>6}               │", code.code);
                println!("  └─────────────────────────────────────┘");
                println!("\n  Web:  http://127.0.0.1:8000/?code={}", code.code);
                println!("  (code stays valid while this server is running)\n");
                account::spawn_pairing_refresh(cfg.clone());
            }
        }
    } else {
        println!("  Pairing token:  {token}");
        println!("\n  Connect your App to: {scheme}://{addr}/?token={token}\n");
    }

    let (pair_host, pair_port, pair_tls) = if saved.is_none() && args.tunnel {
        println!("  Starting Cloudflare Tunnel (public wss access)…");
        let local_url = format!("http://localhost:{}", args.port);
        match tunnel::start_quick_tunnel(&local_url).await {
            Ok(public_host) => {
                println!("  Public tunnel:  https://{public_host}");
                (public_host, 443u16, 1u8)
            }
            Err(e) => {
                tracing::error!("cloudflare tunnel failed: {e}; falling back to LAN pairing");
                (
                    args.pair_host.clone().unwrap_or_else(detect_lan_ip),
                    args.port,
                    if args.tls { 1 } else { 0 },
                )
            }
        }
    } else {
        (
            args.pair_host.clone().unwrap_or_else(detect_lan_ip),
            args.port,
            if args.tls { 1 } else { 0 },
        )
    };
    if saved.is_none() {
        let pair_url = format!("synapse://{pair_host}:{pair_port}?token={token}&tls={pair_tls}");
        println!("  Pairing URL (LAN): {pair_url}");
        println!("  Scan this QR for direct LAN pairing:\n");
        match qr2term::print_qr(&pair_url) {
            Ok(_) => println!(),
            Err(e) => tracing::warn!("could not render pairing QR: {e}"),
        }
    }

    let relay_bridge = if let Some(relay_url) = args
        .relay
        .clone()
        .or_else(|| saved.as_ref().map(|c| c.uplink_url()))
    {
        let (device_id, relay_token) = if let Some(cfg) = &saved {
            if args.relay_device_id.is_none() && args.relay.is_none() {
                (cfg.device_id.clone(), cfg.device_token.clone())
            } else {
                (
                    args.relay_device_id
                        .clone()
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()[..8].to_string()),
                    args.relay_token.clone().unwrap_or_else(|| token.clone()),
                )
            }
        } else {
            (
                args.relay_device_id
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()[..8].to_string()),
                args.relay_token.clone().unwrap_or_else(|| token.clone()),
            )
        };
        if saved.is_none() {
            let connect_host = relay_url
                .replace("wss://", "")
                .replace("ws://", "")
                .replace("/uplink", "")
                .replace("/connect", "");
            let app_connect = format!(
                "synapse://{connect_host}/connect?deviceId={device_id}&token={relay_token}&tls=1"
            );
            println!("  Relay uplink:   {relay_url} (deviceId={device_id})");
            println!("  Relay pair URL: {app_connect}\n");
        }
        Some((relay_url, device_id, relay_token))
    } else {
        None
    };

    tail::spawn(manager.clone());

    tokio::spawn(async move {
        match manager.sync_managed().await {
            n if n > 0 => println!("  Attached {n} existing Claude Code session(s)."),
            _ => {}
        }
    });

    // Account + relay mode only needs localhost; avoids clashing with other LAN services.
    let bind_host = if saved.is_some() && args.host == "0.0.0.0" {
        "127.0.0.1".to_string()
    } else {
        args.host.clone()
    };

    startup::force_stop_existing_server(args.port)?;

    if args.tls {
        let rustls_config = if args.tls_self_signed {
            let sans: Vec<String> = args
                .tls_san
                .as_deref()
                .map(|s| {
                    s.split(',')
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            tls::self_signed_config(
                &sans,
                args.tls_cert_out.as_deref(),
                args.tls_key_out.as_deref(),
            )
            .await?
        } else {
            let cert = args
                .tls_cert
                .as_ref()
                .context("--tls requires --tls-cert (or use --tls-self-signed)")?;
            let key = args
                .tls_key
                .as_ref()
                .context("--tls requires --tls-key (or use --tls-self-signed)")?;
            tls::config_from_files(cert, key).await?
        };
        let bound = bind_addr_rustls(&bind_host, args.port).await?;
        spawn_relay_bridge(relay_bridge, bound.port(), args.tls, &token);
        axum_server::bind_rustls(bound, rustls_config)
            .serve(router.into_make_service())
            .await?;
    } else {
        let (listener, bound) = bind_listener(&bind_host, args.port).await?;
        spawn_relay_bridge(relay_bridge, bound.port(), false, &token);
        axum::serve(listener, router.into_make_service()).await?;
    }
    Ok(())
}

fn spawn_relay_bridge(
    relay_bridge: Option<(String, String, String)>,
    port: u16,
    tls: bool,
    token: &str,
) {
    let Some((relay_url, device_id, relay_token)) = relay_bridge else {
        return;
    };
    let local_ws = format!(
        "{}://127.0.0.1:{port}/?token={token}",
        if tls { "wss" } else { "ws" },
    );
    tracing::info!(%relay_url, %device_id, "relay uplink starting");
    tokio::spawn(async move {
        relay::run_bridge(&relay_url, &device_id, &relay_token, &local_ws).await;
    });
}

async fn bind_listener(host: &str, port: u16) -> Result<(TcpListener, SocketAddr)> {
    for offset in 0..16u16 {
        let p = port.saturating_add(offset);
        let addr: SocketAddr = format!("{host}:{p}")
            .parse()
            .context("invalid bind address")?;
        match TcpListener::bind(addr).await {
            Ok(listener) => {
                if offset > 0 {
                    println!("  Note: port {port} in use — listening on {p} instead.");
                }
                let bound = listener.local_addr()?;
                return Ok((listener, bound));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(e) => return Err(e.into()),
        }
    }
    bail!(
        "ports {port}–{} are in use — stop the other synapse-server (`lsof -i :{port}`) or pass --port",
        port.saturating_add(15)
    );
}

async fn bind_addr_rustls(host: &str, port: u16) -> Result<SocketAddr> {
    for offset in 0..16u16 {
        let p = port.saturating_add(offset);
        let addr: SocketAddr = format!("{host}:{p}")
            .parse()
            .context("invalid bind address")?;
        match TcpListener::bind(addr).await {
            Ok(listener) => {
                if offset > 0 {
                    println!("  Note: port {port} in use — listening on {p} instead.");
                }
                return Ok(listener.local_addr()?);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(e) => return Err(e.into()),
        }
    }
    bail!(
        "ports {port}–{} are in use — stop the other synapse-server or pass --port",
        port.saturating_add(15)
    );
}

fn detect_lan_ip() -> String {
    use std::net::UdpSocket;
    let candidates = ["8.8.8.8:80", "114.114.114.114:80", "1.1.1.1:80"];
    for addr in candidates {
        if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
            if sock.connect(addr).is_ok() {
                if let Ok(local) = sock.local_addr() {
                    let ip = local.ip();
                    if !ip.is_loopback() {
                        return ip.to_string();
                    }
                }
            }
        }
    }
    "127.0.0.1".to_string()
}
