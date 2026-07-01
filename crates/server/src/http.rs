// HTTP + WebSocket server. Mirrors the Node prototype protocol so the same
// mobile client contract applies:
//   WS  /?token=<CODE>            -> bidirectional event stream + commands
//   GET /api/health               -> { ok, sessions }
//   GET /api/sessions?token=CODE  -> list
//   GET /api/pair                 -> { ok }
// Commands over WS: {op:"create"|"send"|"refresh"|"list", ...}

use crate::manager::{CreateOpts, SessionManager};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Query, State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use rand::seq::SliceRandom;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub manager: Arc<SessionManager>,
    pub token: String,
}

fn gen_token() -> String {
    let alphabet: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..6)
        .map(|_| *alphabet.choose(&mut rng).unwrap() as char)
        .collect()
}

pub fn router(manager: Arc<SessionManager>, token: Option<String>) -> (Router, String) {
    let token = token.unwrap_or_else(gen_token);
    let state = AppState {
        manager,
        token: token.clone(),
    };
    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/pair", get(pair))
        .route("/api/sessions", get(sessions))
        .route("/", get(ws_handler))
        .with_state(state);
    (app, token)
}

async fn health(State(s): State<AppState>) -> impl IntoResponse {
    let n = s.manager.list().await.len();
    axum::Json(json!({ "ok": true, "sessions": n }))
}

#[derive(Deserialize)]
struct TokenQ {
    token: Option<String>,
}

async fn pair(State(s): State<AppState>, Query(q): Query<TokenQ>) -> impl IntoResponse {
    if q.token.as_deref() != Some(&s.token) {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            axum::Json(json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    axum::Json(json!({ "ok": true })).into_response()
}

async fn sessions(State(s): State<AppState>, Query(q): Query<TokenQ>) -> impl IntoResponse {
    if q.token.as_deref() != Some(&s.token) {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            axum::Json(json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    let list = s.manager.list().await;
    axum::Json(json!({ "sessions": list })).into_response()
}

async fn ws_handler(
    State(s): State<AppState>,
    Query(q): Query<TokenQ>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    if q.token.as_deref() != Some(&s.token) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |socket| client_loop(s, socket))
}

async fn client_loop(state: AppState, socket: WebSocket) {
    tracing::info!("ws client connected");
    let (mut ws_tx, mut ws_rx) = socket.split();

    // send hello
    let sessions = state.manager.list().await;
    let hello = json!({
        "type": "hello",
        "sessions": sessions,
        "models": state.manager.catalog(),
        "defaultModel": state.manager.default_model_id(),
        "cwds": state.manager.cwds().await,
    });
    let _ = ws_tx.send(Message::Text(hello.to_string())).await;

    // subscribe to manager events
    let mut rx = state.manager.subscribe().await;

    // a shared sender so both the event pump and command loop can write
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Message>(64);
    let out_pump = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if ws_tx.send(m).await.is_err() {
                break;
            }
        }
    });

    // pump manager events -> client via out_tx
    let event_tx = out_tx.clone();
    let event_pump = tokio::spawn(async move {
        while let Some(evt) = rx.recv().await {
            let msg = Message::Text(json!({"type":"event","event":evt}).to_string());
            if event_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    // read commands from client
    while let Some(Ok(msg)) = ws_rx.next().await {
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        let cmd: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let op = cmd.get("op").and_then(|v| v.as_str()).unwrap_or("");
        match op {
            "create" => {
                let opts: CreateOpts =
                    serde_json::from_value(cmd.get("opts").cloned().unwrap_or(json!({})))
                        .unwrap_or(CreateOpts {
                            cwd: None,
                            name: None,
                            model: None,
                            permission_mode: None,
                            agent: None,
                        });
                match state.manager.create(opts).await {
                    Ok(s) => {
                        let _ = out_tx
                            .send(Message::Text(
                                json!({"type":"created","session":s}).to_string(),
                            ))
                            .await;
                    }
                    Err(e) => {
                        let _ = out_tx
                            .send(Message::Text(json!({"type":"error","error":e}).to_string()))
                            .await;
                    }
                }
            }
            "send" => {
                let sid = cmd
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let content = cmd
                    .get("content")
                    .map(|v| {
                        if let Some(s) = v.as_str() {
                            s.to_string()
                        } else {
                            v.to_string()
                        }
                    })
                    .unwrap_or_default();
                tracing::info!(session_id = %sid, content_len = content.len(), op = "send");
                if let Err(e) = state.manager.send(&sid, content).await {
                    let _ = out_tx
                        .send(Message::Text(
                            json!({"type":"error","error":e,"op":"send"}).to_string(),
                        ))
                        .await;
                }
            }
            "refresh" => {
                state.manager.sync_managed().await;
                let list = state.manager.list().await;
                let _ = out_tx
                    .send(Message::Text(
                        json!({"type":"sessions","sessions":list}).to_string(),
                    ))
                    .await;
            }
            "history" => {
                let sid = cmd
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let limit = cmd.get("limit").and_then(|v| v.as_u64()).unwrap_or(400) as usize;
                let (events, found) = state.manager.history(&sid, limit).await;
                let _ = out_tx
                    .send(Message::Text(
                        json!({"type":"history","sessionId":sid,"events":events,"found":found})
                            .to_string(),
                    ))
                    .await;
            }
            "list" => {
                let list = state.manager.list().await;
                let _ = out_tx
                    .send(Message::Text(
                        json!({"type":"sessions","sessions":list}).to_string(),
                    ))
                    .await;
            }
            "stop" => {
                let sid = cmd
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Err(e) = state.manager.stop(&sid).await {
                    let _ = out_tx
                        .send(Message::Text(
                            json!({"type":"error","error":e,"op":"stop"}).to_string(),
                        ))
                        .await;
                }
            }
            "set_model" => {
                let sid = cmd
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let model = cmd
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                if let Err(e) = state.manager.set_model(&sid, model).await {
                    let _ = out_tx
                        .send(Message::Text(
                            json!({"type":"error","error":e,"op":"set_model"}).to_string(),
                        ))
                        .await;
                }
            }
            "set_permission_mode" => {
                let sid = cmd
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mode = cmd.get("mode").and_then(|v| v.as_str()).map(str::to_string);
                if let Err(e) = state.manager.set_permission_mode(&sid, mode).await {
                    let _ = out_tx
                        .send(Message::Text(
                            json!({"type":"error","error":e,"op":"set_permission_mode"})
                                .to_string(),
                        ))
                        .await;
                }
            }
            "permission_response" => {
                let sid = cmd
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let request_id = cmd
                    .get("requestId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                // Accept `behavior:"allow"|"deny"` (or `allow:true/false`).
                let allow = match cmd.get("behavior").and_then(|v| v.as_str()) {
                    Some("allow") => true,
                    Some("deny") => false,
                    _ => cmd.get("allow").and_then(|v| v.as_bool()).unwrap_or(false),
                };
                let message = cmd
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let updated_input = cmd.get("input").cloned();
                if let Err(e) = state
                    .manager
                    .respond_permission(&sid, &request_id, allow, message, updated_input)
                    .await
                {
                    let _ = out_tx
                        .send(Message::Text(
                            json!({"type":"error","error":e,"op":"permission_response"})
                                .to_string(),
                        ))
                        .await;
                }
            }
            "rename" => {
                let sid = cmd
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = cmd
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if !name.is_empty() {
                    if let Err(e) = state.manager.rename(&sid, name).await {
                        let _ = out_tx
                            .send(Message::Text(
                                json!({"type":"error","error":e,"op":"rename"}).to_string(),
                            ))
                            .await;
                    }
                }
            }
            "delete" => {
                let sid = cmd
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Err(e) = state.manager.delete(&sid).await {
                    let _ = out_tx
                        .send(Message::Text(
                            json!({"type":"error","error":e,"op":"delete"}).to_string(),
                        ))
                        .await;
                }
            }
            "pin" => {
                let sid = cmd
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let pinned = cmd.get("pinned").and_then(|v| v.as_bool()).unwrap_or(true);
                if let Err(e) = state.manager.set_pinned(&sid, pinned).await {
                    let _ = out_tx
                        .send(Message::Text(
                            json!({"type":"error","error":e,"op":"pin"}).to_string(),
                        ))
                        .await;
                }
            }
            "archive" => {
                let ids: Vec<String> =
                    if let Some(arr) = cmd.get("sessionIds").and_then(|v| v.as_array()) {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    } else {
                        cmd.get("sessionId")
                            .and_then(|v| v.as_str())
                            .map(|s| vec![s.to_string()])
                            .unwrap_or_default()
                    };
                if ids.is_empty() {
                    continue;
                }
                if let Err(e) = state.manager.archive_many(&ids).await {
                    let _ = out_tx
                        .send(Message::Text(
                            json!({"type":"error","error":e,"op":"archive"}).to_string(),
                        ))
                        .await;
                }
            }
            "unarchive" => {
                let ids: Vec<String> =
                    if let Some(arr) = cmd.get("sessionIds").and_then(|v| v.as_array()) {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    } else {
                        cmd.get("sessionId")
                            .and_then(|v| v.as_str())
                            .map(|s| vec![s.to_string()])
                            .unwrap_or_default()
                    };
                if ids.is_empty() {
                    continue;
                }
                if let Err(e) = state.manager.unarchive_many(&ids).await {
                    let _ = out_tx
                        .send(Message::Text(
                            json!({"type":"error","error":e,"op":"unarchive"}).to_string(),
                        ))
                        .await;
                }
            }
            "refresh_cwds" => {
                let cwds = state.manager.refresh_cwds().await;
                let _ = out_tx
                    .send(Message::Text(
                        json!({"type":"cwds","cwds":cwds}).to_string(),
                    ))
                    .await;
            }
            "register_project" => {
                let path = cmd.get("path").and_then(|v| v.as_str()).unwrap_or("");
                match state.manager.register_project(path).await {
                    Ok(cwds) => {
                        let _ = out_tx
                            .send(Message::Text(
                                json!({"type":"cwds","cwds":cwds}).to_string(),
                            ))
                            .await;
                    }
                    Err(e) => {
                        let _ = out_tx
                            .send(Message::Text(
                                json!({"type":"error","error":e,"op":"register_project"})
                                    .to_string(),
                            ))
                            .await;
                    }
                }
            }
            _ => {}
        }
    }
    out_pump.abort();
    event_pump.abort();
    tracing::info!("ws client disconnected");
}
