use anyhow::{bail, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand::rngs::OsRng;

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hash password: {e}"))?
        .to_string();
    Ok(hash)
}

pub fn verify_password(password: &str, hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(hash).map_err(|e| anyhow::anyhow!("parse hash: {e}"))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

pub fn validate_email(email: &str) -> Result<()> {
    if email.contains('@') && email.len() >= 3 {
        Ok(())
    } else {
        bail!("invalid email")
    }
}

pub fn validate_password(password: &str) -> Result<()> {
    if password.len() >= 6 {
        Ok(())
    } else {
        bail!("password must be at least 6 characters")
    }
}

pub fn new_session_token() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn new_device_id() -> String {
    uuid::Uuid::new_v4().to_string()[..12].to_string()
}

pub fn new_device_token() -> String {
    uuid::Uuid::new_v4().to_string().replace('-', "")
}

pub fn new_connect_token() -> String {
    uuid::Uuid::new_v4().to_string().replace('-', "")
}

pub fn new_pairing_code() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    format!("{:06}", rng.gen_range(0..1_000_000))
}

pub fn new_verification_code() -> String {
    new_pairing_code()
}

pub const EMAIL_CODE_SECS: i64 = 900;
pub const SESSION_DAYS: i64 = 30;
