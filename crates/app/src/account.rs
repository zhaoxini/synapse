//! Relay account API client and app session persistence.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub relay_api: String,
    pub relay_ws: String,
    pub relay_host: String,
    pub relay_port: u16,
    pub relay_tls: bool,
    pub session_token: String,
    pub user_email: String,
    #[serde(default = "default_verified")]
    pub email_verified: bool,
}

fn default_verified() -> bool {
    true
}

impl AppConfig {
    pub fn path() -> PathBuf {
        homedir().join(".synapse").join("app.json")
    }

    pub fn load() -> Result<Option<Self>> {
        let path = Self::path();
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&path).context("read app config")?;
        Ok(Some(
            serde_json::from_str(&raw).context("parse app config")?,
        ))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn clear() -> Result<()> {
        if let Ok(Some(cfg)) = Self::load() {
            let client = reqwest::Client::new();
            let _ = client
                .post(format!("{}/api/v1/auth/logout", cfg.relay_api))
                .header("Authorization", format!("Bearer {}", cfg.session_token))
                .send();
        }
        let path = Self::path();
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
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
            (
                h.to_string(),
                p.parse().unwrap_or(if tls { 443 } else { 80 }),
            )
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
    #[serde(default)]
    email_verified: bool,
    user: UserResp,
    relay_host: String,
    relay_port: u16,
    relay_tls: bool,
}

#[derive(Deserialize)]
struct UserResp {
    email: String,
    #[serde(default)]
    email_verified: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceListItem {
    pub id: String,
    pub name: String,
    pub online: bool,
}

#[derive(Deserialize)]
pub struct ConnectResp {
    pub device_id: String,
    pub connect_token: String,
    pub relay_host: String,
    pub relay_port: u16,
    pub relay_tls: bool,
}

pub struct AccountClient {
    client: reqwest::Client,
    pub cfg: AppConfig,
}

impl AccountClient {
    pub fn from_config(cfg: AppConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            cfg,
        }
    }

    fn cfg_from_auth(
        api: String,
        ws: String,
        auth: AuthResp,
    ) -> AppConfig {
        let verified = auth.email_verified || auth.user.email_verified;
        AppConfig {
            relay_api: api,
            relay_ws: ws,
            relay_host: auth.relay_host,
            relay_port: auth.relay_port,
            relay_tls: auth.relay_tls,
            session_token: auth.session_token,
            user_email: auth.user.email,
            email_verified: verified,
        }
    }

    pub async fn register(relay: &str, email: &str, password: &str, name: &str) -> Result<Self> {
        let (api, ws, _host, _port, _tls) = relay_urls(relay)?;
        let auth: AuthResp = reqwest::Client::new()
            .post(format!("{api}/api/v1/auth/register"))
            .json(&AuthBody {
                email,
                password,
                name,
            })
            .send()
            .await?
            .error_for_status()
            .context("register failed")?
            .json()
            .await?;
        Ok(Self::from_config(Self::cfg_from_auth(api, ws, auth)))
    }

    pub async fn login(relay: &str, email: &str, password: &str) -> Result<Self> {
        let (api, ws, _host, _port, _tls) = relay_urls(relay)?;
        let auth: AuthResp = reqwest::Client::new()
            .post(format!("{api}/api/v1/auth/login"))
            .json(&AuthBody {
                email,
                password,
                name: "",
            })
            .send()
            .await?
            .error_for_status()
            .context("login failed")?
            .json()
            .await?;
        Ok(Self::from_config(Self::cfg_from_auth(api, ws, auth)))
    }

    pub async fn verify_email(&self, code: &str) -> Result<()> {
        self.client
            .post(format!("{}/api/v1/auth/verify-email", self.cfg.relay_api))
            .header(
                "Authorization",
                format!("Bearer {}", self.cfg.session_token),
            )
            .json(&serde_json::json!({ "code": code.trim() }))
            .send()
            .await?
            .error_for_status()
            .context("verify email failed")?;
        Ok(())
    }

    pub async fn resend_verification(&self) -> Result<()> {
        self.client
            .post(format!(
                "{}/api/v1/auth/resend-verification",
                self.cfg.relay_api
            ))
            .header(
                "Authorization",
                format!("Bearer {}", self.cfg.session_token),
            )
            .send()
            .await?
            .error_for_status()
            .context("resend verification failed")?;
        Ok(())
    }

    pub async fn forgot_password(relay: &str, email: &str) -> Result<()> {
        let (api, _ws, _host, _port, _tls) = relay_urls(relay)?;
        reqwest::Client::new()
            .post(format!("{api}/api/v1/auth/forgot-password"))
            .json(&serde_json::json!({ "email": email.trim().to_lowercase() }))
            .send()
            .await?
            .error_for_status()
            .context("forgot password failed")?;
        Ok(())
    }

    pub async fn reset_password(
        relay: &str,
        email: &str,
        code: &str,
        password: &str,
    ) -> Result<()> {
        let (api, _ws, _host, _port, _tls) = relay_urls(relay)?;
        reqwest::Client::new()
            .post(format!("{api}/api/v1/auth/reset-password"))
            .json(&serde_json::json!({
                "email": email.trim().to_lowercase(),
                "code": code.trim(),
                "password": password,
            }))
            .send()
            .await?
            .error_for_status()
            .context("reset password failed")?;
        Ok(())
    }

    pub fn google_oauth_url(relay: &str) -> Result<String> {
        let (api, _ws, _host, _port, _tls) = relay_urls(relay)?;
        Ok(format!("{api}/api/v1/auth/oauth/google"))
    }

    pub async fn list_devices(&self) -> Result<Vec<DeviceListItem>> {
        self.client
            .get(format!("{}/api/v1/devices", self.cfg.relay_api))
            .header(
                "Authorization",
                format!("Bearer {}", self.cfg.session_token),
            )
            .send()
            .await?
            .error_for_status()
            .context("list devices failed")?
            .json()
            .await
            .map_err(Into::into)
    }

    pub async fn connect_device(&self, device_id: &str) -> Result<ConnectResp> {
        self.client
            .post(format!(
                "{}/api/v1/devices/{device_id}/connect",
                self.cfg.relay_api
            ))
            .header(
                "Authorization",
                format!("Bearer {}", self.cfg.session_token),
            )
            .send()
            .await?
            .error_for_status()
            .context("connect device failed")?
            .json()
            .await
            .map_err(Into::into)
    }

    pub async fn claim_pairing_code(&self, code: &str) -> Result<ConnectResp> {
        #[derive(Serialize)]
        struct Body<'a> {
            code: &'a str,
        }
        self.client
            .post(format!("{}/api/v1/pairing-codes/claim", self.cfg.relay_api))
            .header(
                "Authorization",
                format!("Bearer {}", self.cfg.session_token),
            )
            .json(&Body { code })
            .send()
            .await?
            .error_for_status()
            .context("claim pairing code failed")?
            .json()
            .await
            .map_err(Into::into)
    }
}
