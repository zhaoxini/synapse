// SessionManager owns the set of ClaudeSession instances and broadcasts every
// bridge event to all WebSocket subscribers. Each session's turns run on a
// dedicated task so concurrent sessions are independent.

use crate::claude::{
    list_managed, new_session_id, ClaudeBin, ClaudeSession, ManagedEntry, SessionState,
};
use crate::models::{discover_catalog, ModelInfo};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::info;

#[derive(Clone, Debug, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub name: Option<String>,
    pub cwd: String,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub agent: Option<String>,
    pub state: SessionState,
    pub started_at: u64,
    pub attached: bool,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub diff_adds: u32,
    #[serde(default)]
    pub diff_dels: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateOpts {
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
}

struct Entry {
    session: Arc<ClaudeSession>,
    started_at: u64,
    attached: bool,
    tx: mpsc::Sender<TurnMsg>,
}

enum TurnMsg {
    Send(String),
    Stop,
}

pub struct SessionManager {
    bin: ClaudeBin,
    default_cwd: PathBuf,
    default_model: Option<String>,
    catalog: Vec<ModelInfo>,
    cwds: Mutex<Vec<String>>,
    sessions: Mutex<HashMap<String, Entry>>,
    subscribers: Mutex<Vec<mpsc::Sender<Value>>>,
    /// User title overrides (session id → name) from `rename`.
    renames: Mutex<HashMap<String, String>>,
    /// Pinned sessions sort to the top of the list.
    pinned: Mutex<HashSet<String>>,
    /// Soft-hidden sessions (reversible via `unarchive`).
    archived: Mutex<HashSet<String>>,
    /// Session ids (local + cc) hidden by `delete`, so a refresh/attach won't
    /// resurrect them. ponytail: in-memory — a deleted session reappears after a
    /// server restart; persist to ~/.synapse if that ever matters.
    hidden: Mutex<HashSet<String>>,
}

/// Project working dirs Synapse offers in the picker: the git repos among
/// Claude Code's known projects (`~/.claude.json` `projects`). Non-git and
/// deleted paths are dropped to cut noise (`~`, caches, etc.).
/// Expand a leading `~` (the one shell-ism we accept from a typed path) to
/// $HOME. `~user` and everything else pass through verbatim. Discovered-project
/// paths are already absolute, so a manually typed path is the only caller that
/// needs this.
fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix('~') {
        if rest.is_empty() || rest.starts_with('/') {
            if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
                return home.join(rest.trim_start_matches('/'));
            }
        }
    }
    PathBuf::from(p)
}

/// Canonical path string for stable client grouping (expand ~, canonicalize when possible).
fn normalize_path_string(p: &str) -> String {
    let expanded = expand_home(p.trim());
    let path = if expanded.exists() {
        expanded.canonicalize().unwrap_or(expanded)
    } else {
        expanded
    };
    let mut s = path.to_string_lossy().into_owned();
    while s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    s
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct ProjectsFile {
    #[serde(default)]
    paths: Vec<String>,
}

fn projects_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".synapse").join("projects.json"))
        .unwrap_or_else(|| PathBuf::from(".synapse/projects.json"))
}

fn load_manual_projects() -> Vec<String> {
    let path = projects_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<ProjectsFile>(&s).ok())
        .map(|f| f.paths)
        .unwrap_or_default()
}

fn save_manual_projects(paths: &[String]) -> Result<(), String> {
    let path = projects_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let file = ProjectsFile {
        paths: paths.to_vec(),
    };
    let txt = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(&path, txt).map_err(|e| e.to_string())
}

fn merge_projects(discovered: Vec<String>, manual: Vec<String>) -> Vec<String> {
    let mut set = HashSet::new();
    for p in discovered.into_iter().chain(manual) {
        let n = normalize_path_string(&p);
        if !n.is_empty() {
            set.insert(n);
        }
    }
    let mut out: Vec<String> = set.into_iter().collect();
    out.sort();
    out
}

/// ponytail: parses the whole (multi-MB) claude.json once at startup; fine for
/// a boot-time read — revisit only if startup latency ever matters.
fn discover_projects() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let Ok(txt) = std::fs::read_to_string(home.join(".claude.json")) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&txt) else {
        return Vec::new();
    };
    match v.get("projects").and_then(|p| p.as_object()) {
        Some(projects) => projects
            .keys()
            .filter(|p| std::path::Path::new(p).join(".git").exists())
            .cloned()
            .collect(),
        None => Vec::new(),
    }
}

impl SessionManager {
    pub fn new(bin: ClaudeBin, default_cwd: PathBuf, default_model: Option<String>) -> Arc<Self> {
        // Catalog + default come from Claude Code's own config (+ ~/.synapse
        // customizations); `--default-model`/SYNAPSE_DEFAULT_MODEL overrides.
        let (catalog, default) = discover_catalog(default_model);
        let cwds = merge_projects(discover_projects(), load_manual_projects());
        info!(models = catalog.len(), projects = cwds.len(), default = %default, "config ready");
        let default_model = Some(default).filter(|s| !s.is_empty());
        let meta = load_meta_sync();
        Arc::new(Self {
            bin,
            default_cwd,
            default_model,
            catalog,
            cwds: Mutex::new(cwds),
            sessions: Mutex::new(HashMap::new()),
            subscribers: Mutex::new(Vec::new()),
            renames: Mutex::new(meta.renames),
            pinned: Mutex::new(meta.pinned.into_iter().collect()),
            archived: Mutex::new(meta.archived.into_iter().collect()),
            hidden: Mutex::new(meta.hidden.into_iter().collect()),
        })
    }

    /// The selectable model catalog (sent to clients in `hello`).
    pub fn catalog(&self) -> &[ModelInfo] {
        &self.catalog
    }

    /// The default model id (empty string when unset → Claude Code default).
    pub fn default_model_id(&self) -> &str {
        self.default_model.as_deref().unwrap_or("")
    }

    /// Known project working dirs (git repos from Claude Code config), for the
    /// composer's project picker.
    pub async fn cwds(&self) -> Vec<String> {
        self.cwds.lock().await.clone()
    }

    /// Re-scan ~/.claude.json projects, merge with manually registered paths.
    pub async fn refresh_cwds(&self) -> Vec<String> {
        let merged = merge_projects(discover_projects(), load_manual_projects());
        *self.cwds.lock().await = merged.clone();
        merged
    }

    /// Register a project path (persisted under ~/.synapse/projects.json).
    pub async fn register_project(&self, path: &str) -> Result<Vec<String>, String> {
        let norm = normalize_path_string(path);
        if norm.is_empty() {
            return Err("empty path".into());
        }
        if !std::path::Path::new(&norm).exists() {
            return Err(format!("path does not exist: {norm}"));
        }
        let mut manual = load_manual_projects();
        if !manual.iter().any(|p| normalize_path_string(p) == norm) {
            manual.push(norm.clone());
            save_manual_projects(&manual)?;
        }
        let merged = merge_projects(discover_projects(), manual);
        *self.cwds.lock().await = merged.clone();
        Ok(merged)
    }

    pub async fn subscribe(&self) -> mpsc::Receiver<Value> {
        let (tx, rx) = mpsc::channel(256);
        self.subscribers.lock().await.push(tx);
        rx
    }

    pub(crate) async fn broadcast(&self, evt: Value) {
        let mut subs = self.subscribers.lock().await;
        subs.retain(|tx| tx.try_send(evt.clone()).is_ok());
    }

    /// Snapshot every session as (local id, cwd, Claude Code session id, state)
    /// for the transcript tailer. Mirrors `list()`'s lock discipline: grab cheap
    /// handles under the sessions lock, then read the per-session async mutexes
    /// without holding it.
    pub(crate) async fn tail_snapshot(
        &self,
    ) -> Vec<(String, String, Option<String>, SessionState)> {
        let snap: Vec<(String, Arc<ClaudeSession>)> = {
            let sessions = self.sessions.lock().await;
            sessions
                .iter()
                .map(|(id, e)| (id.clone(), e.session.clone()))
                .collect()
        };
        let mut out = Vec::with_capacity(snap.len());
        for (id, session) in snap {
            let cc = session.cc_session_id.lock().await.clone();
            let state = *session.state.lock().await;
            let cwd = session.cwd.to_string_lossy().to_string();
            out.push((id, cwd, cc, state));
        }
        out
    }

    async fn summary(
        &self,
        id: &str,
        session: &Arc<ClaudeSession>,
        started_at: u64,
        attached: bool,
    ) -> SessionSummary {
        let state = *session.state.lock().await;
        let cwd = session.cwd.to_string_lossy().to_string();
        // Title: prefer the transcript's first prompt so each row shows what the
        // session is about. This beats both the old "Interactive" default AND the
        // CLI's auto-generated "<project>-<hash>" name (`claude agents --json`) —
        // both are noise that made the list an unscannable pile. Fall back to an
        // explicit name when there's no transcript yet, then to "New session".
        let sid = session
            .cc_session_id
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| id.to_string());
        let pinned = self.pinned.lock().await.contains(id);
        let archived = self.archived.lock().await.contains(id);
        let (diff_adds, diff_dels) = crate::history::diff_stats(&cwd, &sid).await;
        // A user `rename` wins over the transcript-derived title.
        let name = match self.renames.lock().await.get(id).cloned() {
            Some(n) => Some(n),
            None => crate::history::first_user_text(&cwd, &sid)
                .await
                .or_else(|| session.name.clone())
                .or_else(|| Some("New session".into())),
        };
        SessionSummary {
            id: id.to_string(),
            name,
            cwd,
            model: session.model(),
            permission_mode: session.permission_mode(),
            agent: session.agent.clone(),
            state,
            started_at,
            attached,
            pinned,
            archived,
            diff_adds,
            diff_dels,
        }
    }

    pub async fn list(&self) -> Vec<SessionSummary> {
        // Snapshot cheap handles under the lock, then build summaries WITHOUT
        // holding it: summary() reads a transcript for the title, and that file
        // I/O must not block sends/creates waiting on the sessions mutex.
        // ponytail: re-derives titles on each list(); fine for infrequent list
        // calls + a handful of sessions — cache on the session if it ever grows.
        let snap: Vec<(String, Arc<ClaudeSession>, u64, bool)> = {
            let sessions = self.sessions.lock().await;
            sessions
                .iter()
                .map(|(id, e)| (id.clone(), e.session.clone(), e.started_at, e.attached))
                .collect()
        };
        let mut out = Vec::new();
        for (id, session, started_at, attached) in &snap {
            out.push(self.summary(id, session, *started_at, *attached).await);
        }
        out.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.started_at.cmp(&a.started_at))
        });
        out
    }

    /// Backfill a session's transcript from the Claude Code `.jsonl` store.
    pub async fn history(&self, id: &str, limit: usize) -> (Vec<Value>, bool) {
        let (cwd, cc_id) = {
            let sessions = self.sessions.lock().await;
            match sessions.get(id) {
                Some(e) => (
                    e.session.cwd.to_string_lossy().to_string(),
                    e.session.cc_session_id.lock().await.clone(),
                ),
                None => return (Vec::new(), false),
            }
        };
        // Prefer the persistent Claude Code session id when available, since the
        // transcript file is named after it; fall back to our local id.
        let sid = cc_id.unwrap_or_else(|| id.to_string());
        crate::history::load_transcript(&cwd, &sid, limit).await
    }

    fn spawn_runner(
        self: &Arc<Self>,
        id: String,
        session: Arc<ClaudeSession>,
        mut rx: mpsc::Receiver<TurnMsg>,
    ) {
        let this = self.clone();
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                match msg {
                    TurnMsg::Send(content) => {
                        let (etx, mut erx) = mpsc::channel::<Value>(256);
                        let session = session.clone();
                        let idc = id.clone();
                        let this2 = this.clone();
                        // forward bridge events to subscribers
                        let fwd = tokio::spawn(async move {
                            while let Some(evt) = erx.recv().await {
                                let mut v = evt;
                                if let Some(obj) = v.as_object_mut() {
                                    obj.insert(
                                        "sessionId".into(),
                                        serde_json::Value::String(idc.clone()),
                                    );
                                }
                                this2.broadcast(v).await;
                            }
                        });
                        session.run_turn(&content, &etx).await;
                        drop(etx);
                        let _ = fwd.await;
                    }
                    TurnMsg::Stop => {
                        session.stop().await;
                        this.broadcast(serde_json::json!({
                            "type": "system", "subtype": "turn_stopped", "sessionId": id
                        }))
                        .await;
                    }
                }
            }
        });
    }

    pub async fn create(self: &Arc<Self>, opts: CreateOpts) -> Result<SessionSummary, String> {
        let id = new_session_id();
        let cwd = opts
            .cwd
            .map(|c| expand_home(&c))
            .unwrap_or_else(|| self.default_cwd.clone());
        let session = Arc::new(ClaudeSession::new(
            self.bin.clone(),
            id.clone(),
            cwd,
            opts.name,
            opts.model
                .filter(|s| !s.is_empty())
                .or_else(|| self.default_model.clone()),
            opts.permission_mode,
            opts.agent,
        ));
        let (tx, rx) = mpsc::channel(16);
        let started_at = now_ms();
        let entry = Entry {
            session: session.clone(),
            started_at,
            attached: false,
            tx,
        };
        let summary = self.summary(&id, &session, started_at, false).await;
        self.sessions.lock().await.insert(id.clone(), entry);
        self.spawn_runner(id.clone(), session, rx);
        info!(session_id = %id, "session created");
        self.broadcast(serde_json::json!({
            "type": "system", "subtype": "session_created", "sessionId": id, "session": summary
        }))
        .await;
        Ok(summary)
    }

    /// Switch a session's model. Applies from its next turn (each turn
    /// re-spawns `claude -p`). Broadcasts the updated summary so every client
    /// reflects the change.
    pub async fn set_model(&self, id: &str, model: Option<String>) -> Result<(), String> {
        let (session, started_at, attached) = {
            let sessions = self.sessions.lock().await;
            let e = sessions
                .get(id)
                .ok_or_else(|| "unknown session".to_string())?;
            (e.session.clone(), e.started_at, e.attached)
        };
        session.set_model(model);
        let summary = self.summary(id, &session, started_at, attached).await;
        self.broadcast(serde_json::json!({
            "type": "system", "subtype": "session_updated", "sessionId": id, "session": summary
        }))
        .await;
        Ok(())
    }

    /// Switch a session's permission mode (default/acceptEdits/plan/
    /// bypassPermissions). Applies from its next turn; broadcasts the new summary.
    pub async fn set_permission_mode(&self, id: &str, mode: Option<String>) -> Result<(), String> {
        let (session, started_at, attached) = {
            let sessions = self.sessions.lock().await;
            let e = sessions
                .get(id)
                .ok_or_else(|| "unknown session".to_string())?;
            (e.session.clone(), e.started_at, e.attached)
        };
        session.set_permission_mode(mode);
        let summary = self.summary(id, &session, started_at, attached).await;
        self.broadcast(serde_json::json!({
            "type": "system", "subtype": "session_updated", "sessionId": id, "session": summary
        }))
        .await;
        Ok(())
    }

    /// Deliver a permission decision (allow/deny) to a session's running turn,
    /// answering a `permission_request` the client surfaced.
    pub async fn respond_permission(
        &self,
        id: &str,
        request_id: &str,
        allow: bool,
        message: Option<String>,
        updated_input: Option<Value>,
    ) -> Result<(), String> {
        let session = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(id)
                .ok_or_else(|| "unknown session".to_string())?
                .session
                .clone()
        };
        session
            .respond_permission(request_id, allow, message, updated_input)
            .await;
        Ok(())
    }

    /// Set a sticky title for a session (overrides the transcript-derived name).
    pub async fn rename(&self, id: &str, name: String) -> Result<(), String> {
        let (session, started_at, attached) = {
            let sessions = self.sessions.lock().await;
            let e = sessions
                .get(id)
                .ok_or_else(|| "unknown session".to_string())?;
            (e.session.clone(), e.started_at, e.attached)
        };
        self.renames.lock().await.insert(id.to_string(), name);
        let summary = self.summary(id, &session, started_at, attached).await;
        self.broadcast(serde_json::json!({
            "type": "system", "subtype": "session_updated", "sessionId": id, "session": summary
        }))
        .await;
        self.persist_meta().await;
        Ok(())
    }

    /// Remove a session from the list (interrupting any running turn) and hide it
    /// so a refresh/attach won't bring it back this run. Broadcasts
    /// `session_deleted` so every client drops it.
    pub async fn delete(&self, id: &str) -> Result<(), String> {
        let (tx, session) = {
            let sessions = self.sessions.lock().await;
            let e = sessions
                .get(id)
                .ok_or_else(|| "unknown session".to_string())?;
            (e.tx.clone(), e.session.clone())
        };
        let cc = session.cc_session_id.lock().await.clone();
        let _ = tx.send(TurnMsg::Stop).await; // interrupt a running turn, if any
        self.sessions.lock().await.remove(id);
        {
            let mut hidden = self.hidden.lock().await;
            hidden.insert(id.to_string());
            if let Some(cc) = cc {
                hidden.insert(cc);
            }
        }
        self.broadcast(serde_json::json!({
            "type": "system", "subtype": "session_deleted", "sessionId": id
        }))
        .await;
        self.persist_meta().await;
        Ok(())
    }

    /// Pin a session to the top of the list.
    pub async fn set_pinned(&self, id: &str, pinned: bool) -> Result<(), String> {
        let (session, started_at, attached) = {
            let sessions = self.sessions.lock().await;
            let e = sessions
                .get(id)
                .ok_or_else(|| "unknown session".to_string())?;
            (e.session.clone(), e.started_at, e.attached)
        };
        {
            let mut set = self.pinned.lock().await;
            if pinned {
                set.insert(id.to_string());
            } else {
                set.remove(id);
            }
        }
        let summary = self.summary(id, &session, started_at, attached).await;
        self.broadcast(serde_json::json!({
            "type": "system", "subtype": "session_updated", "sessionId": id, "session": summary
        }))
        .await;
        self.persist_meta().await;
        Ok(())
    }

    /// Soft-hide a session (reversible with `unarchive`).
    pub async fn archive(&self, id: &str) -> Result<(), String> {
        let (session, started_at, attached) = {
            let sessions = self.sessions.lock().await;
            let e = sessions
                .get(id)
                .ok_or_else(|| "unknown session".to_string())?;
            (e.session.clone(), e.started_at, e.attached)
        };
        self.archived.lock().await.insert(id.to_string());
        self.pinned.lock().await.remove(id);
        let summary = self.summary(id, &session, started_at, attached).await;
        self.broadcast(serde_json::json!({
            "type": "system", "subtype": "session_updated", "sessionId": id, "session": summary
        }))
        .await;
        self.persist_meta().await;
        Ok(())
    }

    /// Restore an archived session to the active list.
    pub async fn unarchive(&self, id: &str) -> Result<(), String> {
        let (session, started_at, attached) = {
            let sessions = self.sessions.lock().await;
            let e = sessions
                .get(id)
                .ok_or_else(|| "unknown session".to_string())?;
            (e.session.clone(), e.started_at, e.attached)
        };
        self.archived.lock().await.remove(id);
        let summary = self.summary(id, &session, started_at, attached).await;
        self.broadcast(serde_json::json!({
            "type": "system", "subtype": "session_updated", "sessionId": id, "session": summary
        }))
        .await;
        self.persist_meta().await;
        Ok(())
    }

    /// Archive multiple sessions in one call.
    pub async fn archive_many(&self, ids: &[String]) -> Result<(), String> {
        for id in ids {
            if self.sessions.lock().await.contains_key(id) {
                self.archive(id).await?;
            }
        }
        Ok(())
    }

    /// Restore multiple archived sessions.
    pub async fn unarchive_many(&self, ids: &[String]) -> Result<(), String> {
        for id in ids {
            if self.sessions.lock().await.contains_key(id) {
                self.unarchive(id).await?;
            }
        }
        Ok(())
    }

    pub async fn send(&self, id: &str, content: String) -> Result<(), String> {
        let tx = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(id)
                .ok_or_else(|| "unknown session".to_string())?
                .tx
                .clone()
        };
        // Echo the user message to every subscriber so all devices viewing this
        // session render the question — the turn stream itself never carries it.
        // Broadcast before queueing so it precedes the runner's turn_started.
        self.broadcast(serde_json::json!({
            "type": "user",
            "sessionId": id,
            "message": { "role": "user", "content": [ { "type": "text", "text": content } ] }
        }))
        .await;
        tx.send(TurnMsg::Send(content))
            .await
            .map_err(|e| e.to_string())
    }

    /// Request an interrupt of the current turn for `id`, if one is running.
    pub async fn stop(&self, id: &str) -> Result<(), String> {
        let tx = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(id)
                .ok_or_else(|| "unknown session".to_string())?
                .tx
                .clone()
        };
        // Stop is advisory; ignore a closed channel (no turn running).
        let _ = tx.send(TurnMsg::Stop).await;
        Ok(())
    }

    /// Attach to sessions already running on this machine (`claude agents`).
    pub async fn sync_managed(self: &Arc<Self>) -> usize {
        let entries = match list_managed(&self.bin).await {
            Ok(e) => e,
            Err(_) => return 0,
        };
        let mut count = 0;
        for e in entries {
            if self.attach_managed(e).await.is_some() {
                count += 1;
            }
        }
        count
    }

    async fn attach_managed(self: &Arc<Self>, e: ManagedEntry) -> Option<()> {
        // Background agents (`claude agents` kind=="background") cannot be
        // resumed with `claude -p --resume <id>` — the CLI reports "No
        // conversation found with session ID", the turn produces nothing, and
        // the client sees a response-less send. Only attach resumable sessions.
        if !is_attachable(&e) {
            info!(
                session_id = ?e.session_id.as_deref().or(e.id.as_deref()),
                kind = ?e.kind,
                "skipped non-attachable agent"
            );
            return None;
        }
        let sid = e.session_id.or(e.id)?;
        // A user-deleted session must not be resurrected by a refresh/attach.
        if self.hidden.lock().await.contains(&sid) {
            return None;
        }
        let mut sessions = self.sessions.lock().await;
        // dedupe by Claude Code session id
        for entry in sessions.values() {
            if entry.session.cc_session_id.lock().await.as_deref() == Some(&sid) {
                return None;
            }
        }
        let cwd = e
            .cwd
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_cwd.clone());
        // Leave the name unset unless the user explicitly named it: the summary
        // derives a real title from the transcript's first prompt, so a generic
        // "Interactive" here would just mask it and bring back the wall of
        // identical rows. (Background agents are already filtered above.)
        let name = e.name.clone();
        let session = Arc::new(ClaudeSession::new(
            self.bin.clone(),
            sid.clone(),
            cwd,
            name,
            self.default_model.clone(),
            None,
            None,
        ));
        // seed cc session id so turns resume into the existing conversation
        *session.cc_session_id.lock().await = Some(sid.clone());
        let (tx, rx) = mpsc::channel(16);
        let started_at = e.started_at.unwrap_or_else(now_ms);
        let entry = Entry {
            session: session.clone(),
            started_at,
            attached: true,
            tx,
        };
        let summary = self.summary(&sid, &session, started_at, true).await;
        sessions.insert(sid.clone(), entry);
        drop(sessions);
        info!(session_id = %sid, kind = ?e.kind, "attached managed session");
        self.spawn_runner(sid.clone(), session, rx);
        self.broadcast(serde_json::json!({
            "type": "system", "subtype": "session_created", "sessionId": sid, "session": summary
        }))
        .await;
        Some(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct SessionMetaFile {
    #[serde(default)]
    pinned: Vec<String>,
    #[serde(default)]
    archived: Vec<String>,
    #[serde(default)]
    renames: HashMap<String, String>,
    #[serde(default)]
    hidden: Vec<String>,
}

fn meta_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".synapse").join("session_meta.json"))
        .unwrap_or_else(|| PathBuf::from(".synapse/session_meta.json"))
}

fn load_meta_sync() -> SessionMetaFile {
    let path = meta_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

impl SessionManager {
    async fn persist_meta(&self) {
        let file = SessionMetaFile {
            pinned: self.pinned.lock().await.iter().cloned().collect(),
            archived: self.archived.lock().await.iter().cloned().collect(),
            renames: self.renames.lock().await.clone(),
            hidden: self.hidden.lock().await.iter().cloned().collect(),
        };
        let path = meta_path();
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        if let Ok(txt) = serde_json::to_string_pretty(&file) {
            let _ = tokio::fs::write(path, txt).await;
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A `claude agents` entry is attachable only if it can be resumed by
/// `claude -p --resume <id>`. Background agents cannot, so they're excluded.
fn is_attachable(e: &ManagedEntry) -> bool {
    e.kind.as_deref() != Some("background")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: Option<&str>) -> ManagedEntry {
        ManagedEntry {
            id: Some("x".into()),
            session_id: Some("x".into()),
            cwd: None,
            name: None,
            kind: kind.map(|k| k.into()),
            state: None,
            started_at: None,
        }
    }

    #[test]
    fn background_agents_are_not_attachable() {
        assert!(!is_attachable(&entry(Some("background"))));
    }

    #[test]
    fn interactive_and_unknown_kinds_are_attachable() {
        assert!(is_attachable(&entry(Some("interactive"))));
        assert!(is_attachable(&entry(None)));
    }

    #[test]
    fn expand_home_handles_tilde() {
        // Reads ambient $HOME (always set in dev/CI) rather than mutating it, so it
        // can't race other tests that read HOME in parallel.
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            expand_home("~/code/foo"),
            PathBuf::from(&home).join("code/foo")
        );
        assert_eq!(expand_home("~"), PathBuf::from(&home));
        assert_eq!(expand_home("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(expand_home("~user"), PathBuf::from("~user")); // not a path expansion
    }

    // AC1 (server half): a send must broadcast the user message to subscribers so
    // every device viewing the session renders the question, even though the turn
    // stream never carries it. The echo precedes any turn output.
    #[tokio::test]
    async fn send_broadcasts_user_echo_to_subscribers() {
        let mgr = SessionManager::new(
            ClaudeBin(std::path::PathBuf::from("/nonexistent/claude")),
            std::env::temp_dir(),
            None,
        );
        let summary = mgr
            .create(CreateOpts {
                cwd: Some(std::env::temp_dir().to_string_lossy().to_string()),
                name: None,
                model: None,
                permission_mode: None,
                agent: None,
            })
            .await
            .unwrap();
        // Subscribe AFTER create so the first event we see is the send's echo.
        let mut rx = mgr.subscribe().await;
        mgr.send(&summary.id, "hello world".to_string())
            .await
            .unwrap();
        let evt = rx.recv().await.expect("a broadcast event");
        assert_eq!(evt.get("type").and_then(|v| v.as_str()), Some("user"));
        assert_eq!(
            evt.get("sessionId").and_then(|v| v.as_str()),
            Some(summary.id.as_str())
        );
        assert_eq!(
            evt.pointer("/message/content/0/text")
                .and_then(|v| v.as_str()),
            Some("hello world")
        );
    }

    // Phase 3: rename sets a sticky title; delete removes the session from the list.
    #[tokio::test]
    async fn rename_sets_title_and_delete_removes_session() {
        let mgr = SessionManager::new(
            ClaudeBin(std::path::PathBuf::from("/nonexistent/claude")),
            std::env::temp_dir(),
            None,
        );
        let s = mgr
            .create(CreateOpts {
                cwd: Some(std::env::temp_dir().to_string_lossy().to_string()),
                name: None,
                model: None,
                permission_mode: None,
                agent: None,
            })
            .await
            .unwrap();
        assert_eq!(mgr.list().await.len(), 1);
        mgr.rename(&s.id, "My renamed session".to_string())
            .await
            .unwrap();
        assert_eq!(
            mgr.list().await.first().and_then(|x| x.name.as_deref()),
            Some("My renamed session")
        );
        mgr.delete(&s.id).await.unwrap();
        assert!(mgr.list().await.is_empty());
        assert!(mgr.rename(&s.id, "x".into()).await.is_err()); // gone
    }

    #[tokio::test]
    async fn register_project_persists_and_merges() {
        let dir = std::env::temp_dir().join(format!("synapse-reg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = SessionManager::new(
            ClaudeBin(std::path::PathBuf::from("/nonexistent/claude")),
            std::env::temp_dir(),
            None,
        );
        let path = dir.to_string_lossy().to_string();
        let out = mgr.register_project(&path).await.unwrap();
        assert!(out.iter().any(|p| p.contains("synapse-reg")));
        let again = mgr.register_project(&path).await.unwrap();
        assert_eq!(out.len(), again.len());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
