use crate::auth::{
    hash_password, new_connect_token, new_device_id, new_device_token, new_pairing_code,
    new_session_token, new_verification_code, validate_email, validate_password, verify_password,
    EMAIL_CODE_SECS, SESSION_DAYS,
};
use crate::db::{Db, User};
use crate::email::{send_password_reset, send_verification_code};
use crate::sso::exchange_google_code;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

const CONNECT_TOKEN_SECS: i64 = 300;
const PAIRING_CODE_SECS: i64 = 600;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/v1/auth/register", post(register))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(me))
        .route("/api/v1/auth/verify-email", post(verify_email))
        .route("/api/v1/auth/resend-verification", post(resend_verification))
        .route("/api/v1/auth/forgot-password", post(forgot_password))
        .route("/api/v1/auth/reset-password", post(reset_password))
        .route("/api/v1/auth/change-password", post(change_password))
        .route("/api/v1/auth/oauth/google", get(google_oauth_start))
        .route(
            "/api/v1/auth/oauth/google/callback",
            get(google_oauth_callback),
        )
        .route("/api/v1/devices", get(list_devices).post(register_device))
        .route("/api/v1/devices/:id/connect", post(device_connect))
        .route("/api/v1/pairing-codes", post(create_pairing_code))
        .route("/api/v1/pairing-codes/claim", post(claim_pairing_code))
}

async fn health(State(s): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "devices_online": s.registry.device_count().await,
    }))
}

#[derive(Deserialize)]
pub struct AuthBody {
    pub email: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Serialize)]
pub struct AuthResp {
    pub session_token: String,
    pub user: UserResp,
    pub email_verified: bool,
    pub relay_host: String,
    pub relay_port: u16,
    pub relay_tls: bool,
}

#[derive(Serialize)]
pub struct UserResp {
    pub id: String,
    pub email: String,
    pub name: String,
    pub email_verified: bool,
}

pub fn user_resp(u: &User) -> UserResp {
    UserResp {
        id: u.id.clone(),
        email: u.email.clone(),
        name: u.name.clone(),
        email_verified: u.email_verified,
    }
}

pub fn auth_response(s: &AppState, user: &User, session_token: String) -> AuthResp {
    AuthResp {
        session_token,
        user: user_resp(user),
        email_verified: user.email_verified,
        relay_host: s.public_host.clone(),
        relay_port: s.public_port,
        relay_tls: s.tls,
    }
}

async fn register(State(s): State<AppState>, Json(body): Json<AuthBody>) -> impl IntoResponse {
    match register_inner(&s, body).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => api_error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn register_inner(s: &AppState, body: AuthBody) -> anyhow::Result<AuthResp> {
    let email = body.email.trim().to_lowercase();
    validate_email(&email)?;
    validate_password(&body.password)?;
    if s.db.user_by_email(&email)?.is_some() {
        anyhow::bail!("email already registered");
    }
    let user_id = uuid::Uuid::new_v4().to_string();
    let hash = hash_password(&body.password)?;
    let name = if body.name.trim().is_empty() {
        email.split('@').next().unwrap_or("user").to_string()
    } else {
        body.name.trim().to_string()
    };
    s.db
        .create_user(&user_id, &email, &hash, &name, false)?;
    send_verification_for_user(s, &email).await?;
    let session_token = new_session_token();
    let expires = chrono::Utc::now().timestamp() + SESSION_DAYS * 86400;
    s.db.create_session(&session_token, &user_id, expires)?;
    let user = s.db.user_by_id(&user_id)?.unwrap();
    Ok(auth_response(s, &user, session_token))
}

async fn login(State(s): State<AppState>, Json(body): Json<AuthBody>) -> impl IntoResponse {
    match login_inner(&s, body).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => api_error(StatusCode::UNAUTHORIZED, &e.to_string()),
    }
}

async fn login_inner(s: &AppState, body: AuthBody) -> anyhow::Result<AuthResp> {
    let email = body.email.trim().to_lowercase();
    let (user, hash) =
        s.db.user_by_email(&email)?
            .ok_or_else(|| anyhow::anyhow!("invalid email or password"))?;
    if hash.is_empty() {
        anyhow::bail!("this account uses Google sign-in — use OAuth or reset password");
    }
    if !verify_password(&body.password, &hash)? {
        anyhow::bail!("invalid email or password");
    }
    let session_token = new_session_token();
    let expires = chrono::Utc::now().timestamp() + SESSION_DAYS * 86400;
    s.db.create_session(&session_token, &user.id, expires)?;
    Ok(auth_response(s, &user, session_token))
}

#[derive(Serialize)]
struct DeviceListItem {
    id: String,
    name: String,
    online: bool,
}

async fn list_devices(State(s): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let user_id = match bearer_user(&s, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    if let Err(r) = require_verified_user(&s, &user_id) {
        return r;
    }
    let devices = match s.db.devices_for_user(&user_id) {
        Ok(d) => d,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let online = s.registry.online_ids().await;
    let list: Vec<DeviceListItem> = devices
        .into_iter()
        .map(|d| DeviceListItem {
            id: d.id.clone(),
            name: d.name,
            online: online.contains(&d.id),
        })
        .collect();
    Json(list).into_response()
}

#[derive(Deserialize)]
struct RegisterDeviceBody {
    name: String,
}

#[derive(Serialize)]
struct RegisterDeviceResp {
    id: String,
    name: String,
    device_token: String,
}

async fn register_device(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RegisterDeviceBody>,
) -> impl IntoResponse {
    let user_id = match bearer_user(&s, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    if let Err(r) = require_verified_user(&s, &user_id) {
        return r;
    }
    let name = body.name.trim();
    if name.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "device name required");
    }
    let id = new_device_id();
    let device_token = new_device_token();
    if let Err(e) = s.db.create_device(&id, &user_id, name, &device_token) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    Json(RegisterDeviceResp {
        id,
        name: name.to_string(),
        device_token,
    })
    .into_response()
}

#[derive(Serialize)]
struct ConnectResp {
    device_id: String,
    connect_token: String,
    relay_host: String,
    relay_port: u16,
    relay_tls: bool,
    expires_in: i64,
}

async fn device_connect(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
) -> impl IntoResponse {
    let user_id = match bearer_user(&s, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    if let Err(r) = require_verified_user(&s, &user_id) {
        return r;
    }
    match s.db.device_owned_by(&device_id, &user_id) {
        Ok(true) => {}
        Ok(false) => return api_error(StatusCode::NOT_FOUND, "device not found"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
    match issue_connect_token(&s, &device_id, &user_id) {
        Ok(v) => Json(v).into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(Serialize)]
struct PairingCodeResp {
    code: String,
    expires_in: i64,
}

async fn create_pairing_code(State(s): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let (device_id, device_token) = match device_auth(&s, &headers) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if !s
        .db
        .verify_device_token(&device_id, &device_token)
        .unwrap_or(false)
    {
        return api_error(StatusCode::UNAUTHORIZED, "invalid device credentials");
    }
    let code = new_pairing_code();
    let expires = chrono::Utc::now().timestamp() + PAIRING_CODE_SECS;
    if let Err(e) = s.db.create_pairing_code(&code, &device_id, expires) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    Json(PairingCodeResp {
        code,
        expires_in: PAIRING_CODE_SECS,
    })
    .into_response()
}

#[derive(Deserialize)]
struct ClaimPairingBody {
    code: String,
}

async fn claim_pairing_code(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ClaimPairingBody>,
) -> impl IntoResponse {
    let user_id = match bearer_user(&s, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    if let Err(r) = require_verified_user(&s, &user_id) {
        return r;
    }
    let code = body.code.trim();
    let device_id = match s.db.pairing_code_device(code) {
        Ok(Some(id)) => id,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "invalid or expired pairing code"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    if !s.db.device_owned_by(&device_id, &user_id).unwrap_or(false) {
        return api_error(
            StatusCode::FORBIDDEN,
            "pairing code belongs to another account",
        );
    }
    let _ = s.db.delete_pairing_code(code);
    match issue_connect_token(&s, &device_id, &user_id) {
        Ok(v) => Json(v).into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

fn issue_connect_token(
    s: &AppState,
    device_id: &str,
    user_id: &str,
) -> anyhow::Result<ConnectResp> {
    let token = new_connect_token();
    let expires = chrono::Utc::now().timestamp() + CONNECT_TOKEN_SECS;
    s.db.create_connect_token(&token, device_id, user_id, expires)?;
    Ok(ConnectResp {
        device_id: device_id.to_string(),
        connect_token: token,
        relay_host: s.public_host.clone(),
        relay_port: s.public_port,
        relay_tls: s.tls,
        expires_in: CONNECT_TOKEN_SECS,
    })
}

pub fn api_error(status: StatusCode, msg: &str) -> axum::response::Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

#[allow(clippy::result_large_err)]
pub fn bearer_user(s: &AppState, headers: &HeaderMap) -> Result<String, axum::response::Response> {
    let auth = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| api_error(StatusCode::UNAUTHORIZED, "missing authorization"))?;
    let token = auth
        .strip_prefix("Bearer ")
        .ok_or_else(|| api_error(StatusCode::UNAUTHORIZED, "expected Bearer token"))?;
    s.db.session_user_id(token)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::UNAUTHORIZED, "invalid or expired session"))
}

pub fn require_verified_user(
    s: &AppState,
    user_id: &str,
) -> Result<(), axum::response::Response> {
    let user = s
        .db
        .user_by_id(user_id)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "user not found"))?;
    if user.email_verified {
        Ok(())
    } else {
        Err(api_error(
            StatusCode::FORBIDDEN,
            "email not verified — verify your email before using devices",
        ))
    }
}

#[allow(clippy::result_large_err)]
fn device_auth(
    _s: &AppState,
    headers: &HeaderMap,
) -> Result<(String, String), axum::response::Response> {
    let auth = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| api_error(StatusCode::UNAUTHORIZED, "missing authorization"))?;
    let rest = auth
        .strip_prefix("Device ")
        .ok_or_else(|| api_error(StatusCode::UNAUTHORIZED, "expected Device auth"))?;
    let (device_id, device_token) = rest.split_once(':').ok_or_else(|| {
        api_error(
            StatusCode::UNAUTHORIZED,
            "expected Device deviceId:token format",
        )
    })?;
    Ok((device_id.to_string(), device_token.to_string()))
}

pub fn verify_ws_token(db: &Db, device_id: &str, token: &str) -> Result<bool, anyhow::Error> {
    if db.verify_device_token(device_id, token)? {
        return Ok(true);
    }
    if db.verify_connect_token(token, device_id)? {
        return Ok(true);
    }
    Ok(false)
}

pub fn verify_uplink_token(db: &Db, device_id: &str, token: &str) -> Result<bool, anyhow::Error> {
    db.verify_device_token(device_id, token)
}

async fn send_verification_for_user(s: &AppState, email: &str) -> anyhow::Result<()> {
    let code = new_verification_code();
    let expires = chrono::Utc::now().timestamp() + EMAIL_CODE_SECS;
    s.db.store_email_code(email, "verify", &code, expires)?;
    send_verification_code(s.email.as_ref(), email, &code, s.dev).await
}

#[derive(Serialize)]
struct MeResp {
    user: UserResp,
    email_verified: bool,
}

#[derive(Deserialize)]
struct VerifyEmailBody {
    code: String,
}

#[derive(Deserialize)]
struct ForgotPasswordBody {
    email: String,
}

#[derive(Deserialize)]
struct ResetPasswordBody {
    email: String,
    code: String,
    password: String,
}

#[derive(Deserialize)]
struct ChangePasswordBody {
    current_password: String,
    new_password: String,
}

#[derive(Deserialize)]
struct OAuthCallbackQuery {
    code: String,
    state: String,
}

async fn me(State(s): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let user_id = match bearer_user(&s, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    match s.db.user_by_id(&user_id) {
        Ok(Some(user)) => Json(MeResp {
            email_verified: user.email_verified,
            user: user_resp(&user),
        })
        .into_response(),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "user not found"),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn logout(State(s): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let token = match headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|a| a.strip_prefix("Bearer "))
    {
        Some(t) => t,
        None => return api_error(StatusCode::UNAUTHORIZED, "missing authorization"),
    };
    if let Err(e) = s.db.delete_session(token) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    Json(json!({ "ok": true })).into_response()
}

async fn verify_email(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<VerifyEmailBody>,
) -> impl IntoResponse {
    let user_id = match bearer_user(&s, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let user = match s.db.user_by_id(&user_id) {
        Ok(Some(u)) => u,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "user not found"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    match s.db.verify_email_code(&user.email, "verify", body.code.trim()) {
        Ok(true) => {
            let _ = s.db.set_email_verified(&user_id, true);
            Json(json!({ "ok": true, "email_verified": true })).into_response()
        }
        Ok(false) => api_error(StatusCode::BAD_REQUEST, "invalid or expired verification code"),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn resend_verification(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user_id = match bearer_user(&s, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let user = match s.db.user_by_id(&user_id) {
        Ok(Some(u)) => u,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "user not found"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    if user.email_verified {
        return Json(json!({ "ok": true, "email_verified": true })).into_response();
    }
    match send_verification_for_user(&s, &user.email).await {
        Ok(()) => Json(json!({ "ok": true, "sent": true })).into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn forgot_password(
    State(s): State<AppState>,
    Json(body): Json<ForgotPasswordBody>,
) -> impl IntoResponse {
    let email = body.email.trim().to_lowercase();
    if validate_email(&email).is_ok() {
        if s.db.user_by_email(&email).ok().flatten().is_some() {
            let code = new_verification_code();
            let expires = chrono::Utc::now().timestamp() + EMAIL_CODE_SECS;
            let _ = s.db.store_email_code(&email, "reset", &code, expires);
            let _ = send_password_reset(s.email.as_ref(), &email, &code, s.dev).await;
        }
    }
    Json(json!({ "ok": true })).into_response()
}

async fn reset_password(
    State(s): State<AppState>,
    Json(body): Json<ResetPasswordBody>,
) -> impl IntoResponse {
    let email = body.email.trim().to_lowercase();
    if validate_password(&body.password).is_err() {
        return api_error(StatusCode::BAD_REQUEST, "password must be at least 6 characters");
    }
    match s.db.verify_email_code(&email, "reset", body.code.trim()) {
        Ok(true) => {}
        Ok(false) => return api_error(StatusCode::BAD_REQUEST, "invalid or expired reset code"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
    let (user, _) = match s.db.user_by_email(&email) {
        Ok(Some(u)) => u,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "user not found"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let hash = match hash_password(&body.password) {
        Ok(h) => h,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    if let Err(e) = s.db.update_password(&user.id, &hash) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    Json(json!({ "ok": true })).into_response()
}

async fn change_password(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ChangePasswordBody>,
) -> impl IntoResponse {
    let user_id = match bearer_user(&s, &headers) {
        Ok(u) => u,
        Err(r) => return r,
    };
    if validate_password(&body.new_password).is_err() {
        return api_error(StatusCode::BAD_REQUEST, "password must be at least 6 characters");
    }
    let user = match s.db.user_by_id(&user_id) {
        Ok(Some(u)) => u,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "user not found"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let (_, hash) = match s.db.user_by_email(&user.email) {
        Ok(Some(u)) => u,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "user not found"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    if !verify_password(&body.current_password, &hash).unwrap_or(false) {
        return api_error(StatusCode::UNAUTHORIZED, "current password incorrect");
    }
    let new_hash = match hash_password(&body.new_password) {
        Ok(h) => h,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    if let Err(e) = s.db.update_password(&user.id, &new_hash) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    Json(json!({ "ok": true })).into_response()
}

async fn google_oauth_start(State(s): State<AppState>) -> impl IntoResponse {
    let cfg = match s.oauth.as_ref() {
        Some(c) => c,
        None => return api_error(StatusCode::NOT_IMPLEMENTED, "google oauth not configured"),
    };
    let state = uuid::Uuid::new_v4().to_string();
    let expires = chrono::Utc::now().timestamp() + 600;
    if let Err(e) = s.db.store_oauth_state(&state, expires) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    Redirect::temporary(&cfg.google_auth_url(&state)).into_response()
}

async fn google_oauth_callback(
    State(s): State<AppState>,
    Query(q): Query<OAuthCallbackQuery>,
) -> impl IntoResponse {
    let cfg = match s.oauth.as_ref() {
        Some(c) => c,
        None => return api_error(StatusCode::NOT_IMPLEMENTED, "google oauth not configured"),
    };
    if !s.db.consume_oauth_state(&q.state).unwrap_or(false) {
        return api_error(StatusCode::BAD_REQUEST, "invalid oauth state");
    }
    let info = match exchange_google_code(cfg, &q.code).await {
        Ok(i) => i,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let user = match s.db.user_by_oauth("google", &info.sub).ok().flatten() {
        Some(u) => u,
        None => {
            if let Some((existing, _)) =
                s.db.user_by_email(&info.email.to_lowercase()).ok().flatten()
            {
                let _ = s.db.link_oauth(&existing.id, "google", &info.sub);
                match s.db.user_by_id(&existing.id) {
                    Ok(Some(u)) => u,
                    _ => existing,
                }
            } else {
                let id = uuid::Uuid::new_v4().to_string();
                let name = if info.name.is_empty() {
                    info.email.split('@').next().unwrap_or("user").to_string()
                } else {
                    info.name.clone()
                };
                if let Err(e) = s.db.create_oauth_user(
                    &id,
                    &info.email.to_lowercase(),
                    &name,
                    "google",
                    &info.sub,
                ) {
                    return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
                }
                match s.db.user_by_id(&id) {
                    Ok(Some(u)) => u,
                    _ => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "create user failed"),
                }
            }
        }
    };
    let session_token = new_session_token();
    let expires = chrono::Utc::now().timestamp() + SESSION_DAYS * 86400;
    if let Err(e) = s.db.create_session(&session_token, &user.id, expires) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let _resp = auth_response(&s, &user, session_token.clone());
    let deep_link = format!(
        "synapse://oauth?token={}&email={}",
        urlencoding::encode(&session_token),
        urlencoding::encode(&user.email),
    );
    let html = format!(
        r#"<!DOCTYPE html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Synapse — signed in</title></head><body style="font-family:system-ui,sans-serif;max-width:420px;margin:48px auto;padding:0 16px;text-align:center"><h1>Signed in</h1><p>Welcome, {}.</p><p><a href="{}">Open Synapse app</a></p><p style="color:#666;font-size:14px">If the app does not open, sign in on your phone with the same Google account or copy your session from the developer console.</p><script>try{{location.href="{}"}}catch(e){{}}</script></body></html>"#,
        user.email, deep_link, deep_link,
    );
    Html(html).into_response()
}
