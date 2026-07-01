use crate::auth::{
    hash_password, new_connect_token, new_device_id, new_device_token, new_pairing_code,
    new_session_token, validate_email, validate_password, verify_password, SESSION_DAYS,
};
use crate::db::{Db, User};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
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
struct AuthBody {
    email: String,
    #[serde(default)]
    password: String,
}

#[derive(Serialize)]
struct AuthResp {
    session_token: String,
    user: UserResp,
    relay_host: String,
    relay_port: u16,
    relay_tls: bool,
}

#[derive(Serialize)]
struct UserResp {
    id: String,
    email: String,
    name: String,
}

fn user_resp(u: &User) -> UserResp {
    UserResp {
        id: u.id.clone(),
        email: u.email.clone(),
        name: u.name.clone(),
    }
}

fn auth_response(s: &AppState, user: &User, session_token: String) -> AuthResp {
    AuthResp {
        session_token,
        user: user_resp(user),
        relay_host: s.public_host.clone(),
        relay_port: s.public_port,
        relay_tls: s.tls,
    }
}

/// Public self-registration is disabled; accounts are created by the relay admin.
async fn register(_state: State<AppState>, _body: Json<AuthBody>) -> impl IntoResponse {
    api_error(
        StatusCode::FORBIDDEN,
        "registration disabled — contact admin for an account",
    )
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
    if !verify_password(&body.password, &hash)? {
        anyhow::bail!("invalid email or password");
    }
    let session_token = new_session_token();
    let expires = chrono::Utc::now().timestamp() + SESSION_DAYS * 86400;
    s.db.create_session(&session_token, &user.id, expires)?;
    Ok(auth_response(s, &user, session_token))
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

pub fn api_error(status: StatusCode, msg: &str) -> axum::response::Response {
    (status, Json(json!({ "error": msg }))).into_response()
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

/// Create a user account (admin CLI only).
pub fn create_user_account(
    db: &Db,
    email: &str,
    password: &str,
    name: &str,
) -> anyhow::Result<User> {
    let email = email.trim().to_lowercase();
    validate_email(&email)?;
    validate_password(password)?;
    if db.user_by_email(&email)?.is_some() {
        anyhow::bail!("email already exists: {email}");
    }
    let user_id = uuid::Uuid::new_v4().to_string();
    let hash = hash_password(password)?;
    let name = if name.trim().is_empty() {
        email.split('@').next().unwrap_or("user").to_string()
    } else {
        name.trim().to_string()
    };
    db.create_user(&user_id, &email, &hash, &name)?;
    db.user_by_id(&user_id)?
        .ok_or_else(|| anyhow::anyhow!("create user failed"))
}
