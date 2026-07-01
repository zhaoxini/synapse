//! Google OAuth2 login for Synapse SSO.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
}

impl OAuthConfig {
    pub fn from_env(public_host: &str, tls: bool) -> Option<Self> {
        let client_id = std::env::var("SYNAPSE_GOOGLE_CLIENT_ID").ok()?;
        let client_secret = std::env::var("SYNAPSE_GOOGLE_CLIENT_SECRET").ok()?;
        let scheme = if tls { "https" } else { "http" };
        let redirect_uri = std::env::var("SYNAPSE_OAUTH_REDIRECT_URI").unwrap_or_else(|_| {
            format!("{scheme}://{public_host}/api/v1/auth/oauth/google/callback")
        });
        Some(Self {
            client_id,
            client_secret,
            redirect_uri,
        })
    }

    pub fn google_auth_url(&self, state: &str) -> String {
        let params = HashMap::from([
            ("client_id", self.client_id.as_str()),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", "openid email profile"),
            ("state", state),
            ("access_type", "online"),
            ("prompt", "select_account"),
        ]);
        let qs: String = params
            .iter()
            .map(|(k, v)| format!("{k}={}", urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        format!("https://accounts.google.com/o/oauth2/v2/auth?{qs}")
    }
}

#[derive(Debug, Deserialize)]
struct TokenResp {
    access_token: String,
}

#[derive(Debug, Clone)]
pub struct GoogleUserInfo {
    pub sub: String,
    pub email: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct GoogleUserInfoRaw {
    sub: String,
    email: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    email_verified: bool,
}

pub async fn exchange_google_code(cfg: &OAuthConfig, code: &str) -> Result<GoogleUserInfo> {
    let client = reqwest::Client::new();
    let body = format!(
        "code={}&client_id={}&client_secret={}&redirect_uri={}&grant_type=authorization_code",
        urlencoding::encode(code),
        urlencoding::encode(&cfg.client_id),
        urlencoding::encode(&cfg.client_secret),
        urlencoding::encode(&cfg.redirect_uri),
    );
    let token: TokenResp = client
        .post("https://oauth2.googleapis.com/token")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .context("google token request")?
        .error_for_status()
        .context("google token exchange failed")?
        .json()
        .await
        .context("parse google token response")?;

    let info: GoogleUserInfoRaw = client
        .get("https://openidconnect.googleapis.com/v1/userinfo")
        .bearer_auth(token.access_token)
        .send()
        .await
        .context("google userinfo request")?
        .error_for_status()
        .context("google userinfo failed")?
        .json()
        .await
        .context("parse google userinfo")?;

    if !info.email_verified {
        anyhow::bail!("google account email not verified");
    }
    Ok(GoogleUserInfo {
        sub: info.sub,
        email: info.email,
        name: info.name,
    })
}
