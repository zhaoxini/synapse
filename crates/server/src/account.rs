//! Relay account API client and local config persistence.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub relay_api: String,
    pub relay_ws: String,
    pub relay_host: String,
    pub relay_port: u16,
    pub relay_tls: bool,
    pub session_token: String,
    pub user_email: String,
    pub device_id: String,
    pub device_token: String,
    pub device_name: String,
}

impl Config {
    pub fn path() -> PathBuf {
        homedir()
            .join(".synapse")
            .join("config.json")
    }

    pub fn load() -> Result<Option<Self>> {
        let path = Self::path();
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&path).context("read config")?;
        Ok(Some(serde_json::from_str(&raw).context("parse config")?))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(path, raw)?;
        Ok(())
    }

    pub fn uplink_url(&self) -> String {
        format!("{}/uplink", self.relay_ws.trim_end_matches('/'))
    }
}

fn homedir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn relay_urls(relay: &str) -> Result<(String, String, String, u16, bool)> {
    let raw = relay.trim().trim_end_matches('/');
    let tls = raw.starts_with("wss://") || raw.starts_with("https://");
    let ws_base = if raw.starts_with("https://") {
        raw.replacen("https://", "wss://", 1)
    } else if raw.starts_with("http://") {
        raw.replacen("http://", "ws://", 1)
    } else if raw.starts_with("wss://") || raw.starts_with("ws://") {
        raw.to_string()
    } else {
        format!("wss://{raw}")
    };
    let api_base = ws_base
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1);
    let host_port = ws_base
        .trim_start_matches("wss://")
        .trim_start_matches("ws://");
    let (host, port) = if let Some((h, p)) = host_port.rsplit_once(':') {
        if p.chars().all(|c| c.is_ascii_digit()) {
            (h.to_string(), p.parse().unwrap_or(if tls { 443 } else { 80 }))
        } else {
            (host_port.to_string(), if tls { 443 } else { 80 })
        }
    } else {
        (host_port.to_string(), if tls { 443 } else { 80 })
    };
    Ok((api_base, ws_base, host, port, tls))
}

#[derive(Serialize)]
struct AuthBody<'a> {
    email: &'a str,
    password: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    name: &'a str,
}

#[derive(Deserialize)]
struct AuthResp {
    session_token: String,
    user: UserResp,
    relay_host: String,
    relay_port: u16,
    relay_tls: bool,
}

#[derive(Deserialize)]
struct UserResp {
    email: String,
}

#[derive(Serialize)]
struct DeviceBody<'a> {
    name: &'a str,
}

#[derive(Deserialize)]
struct DeviceResp {
    id: String,
    device_token: String,
}

#[derive(Deserialize)]
pub struct PairingCodeResp {
    pub code: String,
    pub expires_in: i64,
}

pub async fn register_account(
    relay: &str,
    email: &str,
    password: &str,
    name: &str,
    device_name: &str,
) -> Result<Config> {
    let (api, ws, host, port, tls) = relay_urls(relay)?;
    let client = reqwest::Client::new();
    let auth: AuthResp = client
        .post(format!("{api}/api/v1/auth/register"))
        .json(&AuthBody {
            email,
            password,
            name,
        })
        .send()
        .await
        .context("register request")?
        .error_for_status()
        .context("register failed")?
        .json()
        .await
        .context("register response")?;
    register_device(&client, &api, &auth.session_token, device_name, ws, host, port, tls, email)
        .await
}

pub async fn login_account(
    relay: &str,
    email: &str,
    password: &str,
    device_name: &str,
) -> Result<Config> {
    let (api, ws, host, port, tls) = relay_urls(relay)?;
    let client = reqwest::Client::new();
    let auth: AuthResp = client
        .post(format!("{api}/api/v1/auth/login"))
        .json(&AuthBody {
            email,
            password,
            name: "",
        })
        .send()
        .await
        .context("login request")?
        .error_for_status()
        .context("login failed")?
        .json()
        .await
        .context("login response")?;
    register_device(&client, &api, &auth.session_token, device_name, ws, host, port, tls, email)
        .await
}

async fn register_device(
    client: &reqwest::Client,
    api: &str,
    session_token: &str,
    device_name: &str,
    ws: String,
    host: String,
    port: u16,
    tls: bool,
    email: &str,
) -> Result<Config> {
    let dev: DeviceResp = client
        .post(format!("{api}/api/v1/devices"))
        .header("Authorization", format!("Bearer {session_token}"))
        .json(&DeviceBody { name: device_name })
        .send()
        .await
        .context("register device")?
        .error_for_status()
        .context("register device failed")?
        .json()
        .await?;
    Ok(Config {
        relay_api: api.to_string(),
        relay_ws: ws,
        relay_host: host,
        relay_port: port,
        relay_tls: tls,
        session_token: session_token.to_string(),
        user_email: email.to_string(),
        device_id: dev.id,
        device_token: dev.device_token,
        device_name: device_name.to_string(),
    })
}

pub async fn create_pairing_code(cfg: &Config) -> Result<PairingCodeResp> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/pairing-codes", cfg.relay_api))
        .header(
            "Authorization",
            format!("Device {}:{}", cfg.device_id, cfg.device_token),
        )
        .send()
        .await
        .context("pairing code request")?
        .error_for_status()
        .context("pairing code failed")?
        .json()
        .await
        .context("pairing code response")?;
    Ok(resp)
}

pub fn default_device_name() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "My Computer".to_string())
}

pub fn default_relay_url() -> Option<String> {
    std::env::var("SYNAPSE_RELAY").ok().filter(|s| !s.trim().is_empty())
}

/// First-run interactive setup: email + password only. Relay comes from
/// `SYNAPSE_RELAY` or a one-time prompt. Tries login first, then register.
pub async fn interactive_setup() -> Result<Config> {
    println!("\n  Welcome to Synapse — first-time setup\n");
    let relay = match default_relay_url() {
        Some(u) => u,
        None => {
            let url = read_line("Relay server [wss://relay.example.com]: ")?;
            if url.is_empty() {
                bail!("relay URL required (set SYNAPSE_RELAY or enter it now)");
            }
            url
        }
    };
    let email = read_line("Email: ")?;
    if email.is_empty() || !email.contains('@') {
        bail!("valid email required");
    }
    let password = read_password("Password: ")?;
    let device_name = default_device_name();
    println!("\n  Signing in…\n");
    match login_account(&relay, &email, &password, &device_name).await {
        Ok(cfg) => {
            cfg.save()?;
            Ok(cfg)
        }
        Err(login_err) => {
            tracing::debug!("login failed: {login_err}; trying register");
            match register_account(&relay, &email, &password, "", &device_name).await {
                Ok(cfg) => {
                    cfg.save()?;
                    Ok(cfg)
                }
                Err(reg_err) => {
                    bail!("sign-in failed ({login_err}). Could not create account either ({reg_err}).");
                }
            }
        }
    }
}

pub fn read_line(prompt: &str) -> Result<String> {
    use std::io::{self, Write};
    print!("{prompt}");
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

pub fn read_password(prompt: &str) -> Result<String> {
    let s = read_line(prompt)?;
    if s.is_empty() {
        bail!("password required");
    }
    Ok(s)
}
