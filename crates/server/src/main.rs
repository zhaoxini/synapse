mod account;
mod claude;
mod history;
mod http;
mod manager;
mod models;
mod relay;
mod tail;
mod tls;
mod tunnel;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use manager::SessionManager;
use std::net::SocketAddr;
use std::path::PathBuf;

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
        None | Some(Commands::Run) => run_server(cli.run).await,
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
            let cfg = account::Config::load()?.context("not logged in — run synapse-server login first")?;
            let code = account::create_pairing_code(&cfg).await?;
            println!("\n  Pairing code:  {}\n", code.code);
            println!("  Expires in {} seconds.", code.expires_in);
            println!("  Enter this code in the Synapse app (same account: {}).\n", cfg.user_email);
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
    let (router, token) = http::router(manager.clone(), args.token);

    let addr: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;
    let scheme = if args.tls { "wss" } else { "ws" };

    println!("\n  Synapse server is running.\n");
    println!("  Claude binary:  {}", bin.0.display());
    println!("  Working dir:    {}", cwd.display());
    println!("  Pairing token:  {token}");
    if args.tls {
        println!(
            "  TLS:            enabled ({})",
            if args.tls_self_signed {
                "self-signed"
            } else {
                "provided cert"
            }
        );
    }
    println!("\n  Connect your App to: {scheme}://{addr}/?token={token}\n");

    let (pair_host, pair_port, pair_tls) = if args.tunnel {
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
    let pair_url = format!("synapse://{pair_host}:{pair_port}?token={token}&tls={pair_tls}");
    println!("  Pairing URL (LAN): {pair_url}");
    println!("  Scan this QR for direct LAN pairing:\n");
    match qr2term::print_qr(&pair_url) {
        Ok(_) => println!(),
        Err(e) => tracing::warn!("could not render pairing QR: {e}"),
    }

    if let Some(cfg) = &saved {
        println!("  Account:        {} ({})", cfg.user_email, cfg.device_name);
        println!("  Relay device:   {}", cfg.device_id);
        if let Ok(code) = account::create_pairing_code(cfg).await {
            println!("  Pairing code:   {}  (enter in app, expires in {}s)", code.code, code.expires_in);
        }
        println!("  Or sign in on the app with the same account to see this device in your list.\n");
    }

    let relay_url = args
        .relay
        .clone()
        .or_else(|| saved.as_ref().map(|c| c.uplink_url()));
    if let Some(relay_url) = relay_url {
        let (device_id, relay_token, connect_host) = if let Some(cfg) = &saved {
            if args.relay_device_id.is_none() && args.relay.is_none() {
                (
                    cfg.device_id.clone(),
                    cfg.device_token.clone(),
                    cfg.relay_host.clone(),
                )
            } else {
                (
                    args.relay_device_id.clone().unwrap_or_else(|| {
                        uuid::Uuid::new_v4().to_string()[..8].to_string()
                    }),
                    args.relay_token.clone().unwrap_or_else(|| token.clone()),
                    relay_url
                        .replace("wss://", "")
                        .replace("ws://", "")
                        .replace("/uplink", "")
                        .replace("/connect", ""),
                )
            }
        } else {
            (
                args.relay_device_id.clone().unwrap_or_else(|| {
                    uuid::Uuid::new_v4().to_string()[..8].to_string()
                }),
                args.relay_token.clone().unwrap_or_else(|| token.clone()),
                relay_url
                    .replace("wss://", "")
                    .replace("ws://", "")
                    .replace("/uplink", "")
                    .replace("/connect", ""),
            )
        };
        let local_ws = format!(
            "{}://127.0.0.1:{}/?token={token}",
            if args.tls { "wss" } else { "ws" },
            args.port
        );
        let app_connect = format!(
            "synapse://{connect_host}/connect?deviceId={device_id}&token={relay_token}&tls=1"
        );
        println!("  Relay uplink:   {relay_url} (deviceId={device_id})");
        println!("  Relay pair URL: {app_connect}\n");
        let relay_url = relay_url.clone();
        let device_id = device_id.clone();
        let relay_token = relay_token.clone();
        let local_ws = local_ws.clone();
        tokio::spawn(async move {
            relay::run_bridge(&relay_url, &device_id, &relay_token, &local_ws).await;
        });
    }

    tail::spawn(manager.clone());

    tokio::spawn(async move {
        match manager.sync_managed().await {
            n if n > 0 => println!("  Attached {n} existing Claude Code session(s)."),
            _ => {}
        }
    });

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
        axum_server::bind_rustls(addr, rustls_config)
            .serve(router.into_make_service())
            .await?;
    } else {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, router.into_make_service()).await?;
    }
    Ok(())
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
