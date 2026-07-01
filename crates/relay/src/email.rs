//! Outbound email for verification and password reset (SMTP).

use anyhow::{Context, Result};
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

#[derive(Clone, Debug)]
pub struct EmailConfig {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_user: String,
    pub smtp_pass: String,
    pub from: String,
}

impl EmailConfig {
    pub fn from_env() -> Option<Self> {
        let host = std::env::var("SYNAPSE_SMTP_HOST").ok()?;
        let user = std::env::var("SYNAPSE_SMTP_USER").ok()?;
        let pass = std::env::var("SYNAPSE_SMTP_PASS").ok()?;
        let from = std::env::var("SYNAPSE_SMTP_FROM").unwrap_or_else(|_| user.clone());
        let port = std::env::var("SYNAPSE_SMTP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(465);
        Some(Self {
            smtp_host: host,
            smtp_port: port,
            smtp_user: user,
            smtp_pass: pass,
            from,
        })
    }

    pub fn configured(&self) -> bool {
        !self.smtp_host.is_empty() && !self.smtp_user.is_empty()
    }
}

pub async fn send_email(cfg: &EmailConfig, to: &str, subject: &str, body: &str) -> Result<()> {
    let email = Message::builder()
        .from(cfg.from.parse().context("invalid SYNAPSE_SMTP_FROM")?)
        .to(to.parse().context("invalid recipient email")?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())?;

    let creds = Credentials::new(cfg.smtp_user.clone(), cfg.smtp_pass.clone());
    let mailer = AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.smtp_host)?
        .port(cfg.smtp_port)
        .credentials(creds)
        .build();

    mailer.send(email).await.context("send smtp email")?;
    Ok(())
}

pub async fn send_verification_code(
    cfg: Option<&EmailConfig>,
    to: &str,
    code: &str,
    dev_log: bool,
) -> Result<()> {
    let subject = "Synapse — verify your email";
    let body = format!(
        "Your Synapse verification code is: {code}\n\nIt expires in 15 minutes.\n"
    );
    if let Some(cfg) = cfg {
        send_email(cfg, to, subject, &body).await
    } else if dev_log {
        tracing::warn!(email = %to, code = %code, "SMTP not configured — verification code (dev only)");
        Ok(())
    } else {
        anyhow::bail!("email delivery not configured on relay (set SYNAPSE_SMTP_* env vars)")
    }
}

pub async fn send_password_reset(
    cfg: Option<&EmailConfig>,
    to: &str,
    code: &str,
    dev_log: bool,
) -> Result<()> {
    let subject = "Synapse — password reset code";
    let body = format!(
        "Your Synapse password reset code is: {code}\n\nIt expires in 15 minutes.\n"
    );
    if let Some(cfg) = cfg {
        send_email(cfg, to, subject, &body).await
    } else if dev_log {
        tracing::warn!(email = %to, code = %code, "SMTP not configured — reset code (dev only)");
        Ok(())
    } else {
        anyhow::bail!("email delivery not configured on relay (set SYNAPSE_SMTP_* env vars)")
    }
}
