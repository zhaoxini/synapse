// Claude Code bridge: drives a `claude -p` session per turn over the supported
// stream-json transport. One logical remote session = one Claude Code session
// id, resumed across turns. Mirrors the Node prototype's behavior including the
// automatic fallback to buffered `--output-format json` when stream-json yields
// nothing (some model gateways drop it).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Clone)]
pub struct ClaudeBin(pub PathBuf);

impl ClaudeBin {
    pub fn resolve(explicit: Option<&PathBuf>) -> Self {
        if let Some(p) = explicit {
            if p.exists() {
                return Self(p.clone());
            }
        }
        for c in [
            std::env::var_os("CLAUDE_BIN").map(PathBuf::from),
            homedir().map(|h| h.join(".hermes/node/bin/claude")),
            homedir().map(|h| h.join(".claude/local/claude")),
            Some(PathBuf::from("/usr/local/bin/claude")),
            Some(PathBuf::from("/opt/homebrew/bin/claude")),
        ]
        .into_iter()
        .flatten()
        {
            if c.exists() {
                return Self(c);
            }
        }
        Self(PathBuf::from("claude"))
    }
}

fn homedir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// One logical remote session.
pub struct ClaudeSession {
    pub id: String,
    pub cwd: PathBuf,
    pub name: Option<String>,
    /// Selected model id passed to `--model` (empty/None = Claude Code default).
    /// Behind a sync mutex so it can be switched mid-session; the next turn's
    /// `base_args` reads the latest value. ponytail: std Mutex, only ever
    /// lock+clone (never held across `.await`).
    pub model: std::sync::Mutex<Option<String>>,
    /// Permission mode passed to `--permission-mode` (None → Claude Code default).
    /// Mutable mid-session like `model`. ponytail: std Mutex, lock+clone only.
    pub permission_mode: std::sync::Mutex<Option<String>>,
    pub agent: Option<String>,
    pub bin: ClaudeBin,
    /// Claude Code's persistent session id, captured from the first turn.
    pub cc_session_id: tokio::sync::Mutex<Option<String>>,
    pub state: tokio::sync::Mutex<SessionState>,
    /// Handle to the currently-running turn child, if any. Stored behind an
    /// Arc so Stop (from a different task) can kill the live process.
    child: tokio::sync::Mutex<Option<Arc<tokio::sync::Mutex<Option<Child>>>>>,
    /// While a streaming turn runs, lines pushed here are written to claude's
    /// stdin (used for `control_response` permission answers). None between turns.
    stdin_tx: tokio::sync::Mutex<Option<mpsc::Sender<String>>>,
    /// Pending permission prompts (request_id → original tool input) awaiting a
    /// client decision; lets a plain allow echo the input back as `updatedInput`.
    pending_perms: tokio::sync::Mutex<HashMap<String, Value>>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Idle,
    Busy,
    Error,
}

impl ClaudeSession {
    pub fn new(
        bin: ClaudeBin,
        id: String,
        cwd: PathBuf,
        name: Option<String>,
        model: Option<String>,
        permission_mode: Option<String>,
        agent: Option<String>,
    ) -> Self {
        Self {
            id,
            cwd,
            name,
            model: std::sync::Mutex::new(model),
            permission_mode: std::sync::Mutex::new(permission_mode),
            agent,
            bin,
            cc_session_id: tokio::sync::Mutex::new(None),
            state: tokio::sync::Mutex::new(SessionState::Idle),
            child: tokio::sync::Mutex::new(None),
            stdin_tx: tokio::sync::Mutex::new(None),
            pending_perms: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Current model id, if any. Empty selection is stored as `None`.
    pub fn model(&self) -> Option<String> {
        self.model.lock().unwrap().clone()
    }

    /// Switch the model. Takes effect on the next turn (each turn re-spawns
    /// `claude -p --model … --resume`), so a running turn is unaffected.
    pub fn set_model(&self, model: Option<String>) {
        *self.model.lock().unwrap() = model.filter(|s| !s.is_empty());
    }

    /// Current permission mode, if any (None → Claude Code default).
    pub fn permission_mode(&self) -> Option<String> {
        self.permission_mode.lock().unwrap().clone()
    }

    /// Switch the permission mode. Applies on the next turn (each turn re-spawns
    /// `claude -p`), mirroring `set_model`.
    pub fn set_permission_mode(&self, mode: Option<String>) {
        *self.permission_mode.lock().unwrap() = mode.filter(|s| !s.is_empty());
    }

    fn base_args(&self, streaming: bool, cc_sid: &Option<String>) -> Vec<String> {
        let mut args = vec!["-p".into(), "--verbose".into()];
        if streaming {
            args.push("--input-format".into());
            args.push("stream-json".into());
            args.push("--output-format".into());
            args.push("stream-json".into());
            args.push("--include-partial-messages".into());
        } else {
            args.push("--output-format".into());
            args.push("json".into());
        }
        if let Some(m) = self.permission_mode.lock().unwrap().clone() {
            args.push("--permission-mode".into());
            args.push(m);
        }
        if streaming {
            // Route tool-permission prompts to us over the stdio control protocol
            // so the client can approve/deny (handled in read_stream). Harmless in
            // bypass mode (claude never prompts then).
            args.push("--permission-prompt-tool".into());
            args.push("stdio".into());
        }
        if let Some(m) = self.model.lock().unwrap().clone() {
            args.push("--model".into());
            args.push(m);
        }
        if let Some(a) = &self.agent {
            args.push("--agent".into());
            args.push(a.clone());
        }
        if let Some(sid) = cc_sid {
            args.push("--resume".into());
            args.push(sid.clone());
        }
        args
    }

    /// Run one turn, streaming events to `tx`. Returns the number of
    /// substantive events produced. Tries stream-json first; falls back to
    /// buffered json if the gateway emits nothing.
    pub async fn run_turn(&self, content: &str, tx: &mpsc::Sender<Value>) -> usize {
        let t0 = Instant::now();
        *self.state.lock().await = SessionState::Busy;
        let _ = tx
            .send(serde_json::json!({
                "type": "system", "subtype": "turn_started", "sessionId": self.id
            }))
            .await;

        let cc = self.cc_session_id.lock().await.clone();
        tracing::info!(session_id = %self.id, cc_sid = ?cc, content_len = content.len(), "turn started");
        let produced = self.exec_turn(content, true, &cc, tx).await;
        let produced = if produced == 0 {
            tracing::info!(session_id = %self.id, "stream-json yielded nothing, falling back to json");
            let _ = tx
                .send(serde_json::json!({
                    "type": "system", "subtype": "fallback_to_json", "sessionId": self.id
                }))
                .await;
            self.exec_turn(content, false, &cc, tx).await
        } else {
            produced
        };

        let failed = produced == 0;
        if failed {
            // The subprocess produced no conversational output (spawn failure,
            // immediate crash, or a fatal provider/auth error). Emit an
            // explicit error line so the client can surface it instead of
            // spinning forever, then signal the turn is over.
            let _ = tx
                .send(serde_json::json!({
                    "type": "stderr", "sessionId": self.id,
                    "text": "Turn failed: the Claude CLI produced no output (check auth, API key, and that the gateway/model is reachable)."
                }))
                .await;
            let _ = tx
                .send(serde_json::json!({
                    "type": "system", "subtype": "bridge_error",
                    "sessionId": self.id,
                    "error": "no output from Claude CLI"
                }))
                .await;
        }
        tracing::info!(
            session_id = %self.id,
            produced,
            elapsed_ms = t0.elapsed().as_millis(),
            failed,
            "turn finished"
        );
        // Always terminate the turn so the client leaves the busy state.
        let _ = tx
            .send(serde_json::json!({
                "type": "system", "subtype": "turn_stopped", "sessionId": self.id
            }))
            .await;
        *self.state.lock().await = if failed {
            SessionState::Error
        } else {
            SessionState::Idle
        };
        produced
    }

    async fn exec_turn(
        &self,
        content: &str,
        streaming: bool,
        cc_sid: &Option<String>,
        tx: &mpsc::Sender<Value>,
    ) -> usize {
        let args = self.base_args(streaming, cc_sid);
        let mut cmd = Command::new(&self.bin.0);
        cmd.args(&args)
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(session_id = %self.id, error = %e, "failed to spawn claude");
                let _ = tx
                    .send(serde_json::json!({
                        "type": "system", "subtype": "bridge_error",
                        "sessionId": self.id, "error": e.to_string()
                    }))
                    .await;
                return 0;
            }
        };

        let stdin = child.stdin.take();
        if streaming {
            // Keep stdin OPEN for the whole turn so we can write `control_response`
            // lines back when claude asks for tool permission (`--permission-prompt-tool
            // stdio`). A writer task owns stdin: it sends the user turn first, then
            // forwards any control responses pushed on `stdin_tx`. The reader closes
            // `stdin_tx` on the turn's `result` frame, dropping stdin (EOF) so claude
            // exits. (The old path dropped stdin immediately, which made interactive
            // permission prompts impossible.)
            let (line_tx, line_rx) = mpsc::channel::<String>(16);
            *self.stdin_tx.lock().await = Some(line_tx);
            let user_line = serde_json::json!({
                "type": "user",
                "message": { "role": "user", "content": [ { "type": "text", "text": content } ] }
            })
            .to_string();
            if let Some(stdin) = stdin {
                tokio::spawn(stdin_writer(stdin, user_line, line_rx));
            }
        } else if let Some(mut stdin) = stdin {
            // Buffered fallback: no interactive control channel; write + close.
            let _ = stdin.write_all(content.as_bytes()).await;
            let _ = stdin.flush().await;
            drop(stdin);
        }

        // Publish the child (stdin already taken) so Stop can kill it. The
        // reader owns the stdout/stderr pipes and the wait; Stop only needs a
        // live reference to call start_kill on.
        let cell: Arc<tokio::sync::Mutex<Option<Child>>> =
            Arc::new(tokio::sync::Mutex::new(Some(child)));
        {
            let mut slot = self.child.lock().await;
            *slot = Some(cell.clone());
        }

        let produced = if streaming {
            self.read_stream(cell.clone(), tx).await
        } else {
            self.read_json(cell.clone(), tx).await
        };
        // Turn done: close the stdin channel (drops stdin if the reader didn't
        // already) and forget any unanswered permission prompts.
        *self.stdin_tx.lock().await = None;
        self.pending_perms.lock().await.clear();
        // clear the handle slot now that the turn is done
        {
            let mut slot = self.child.lock().await;
            *slot = None;
        }
        produced
    }

    /// Surface a `can_use_tool` permission prompt to clients and remember the
    /// pending input so a plain "allow" can echo it back as `updatedInput`.
    async fn emit_permission_request(&self, evt: &Value, tx: &mpsc::Sender<Value>) {
        let request_id = evt
            .get("request_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let req = evt.get("request").cloned().unwrap_or_default();
        let input = req
            .get("input")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        self.pending_perms
            .lock()
            .await
            .insert(request_id.clone(), input.clone());
        let _ = tx
            .send(serde_json::json!({
                "type": "permission_request",
                "requestId": request_id,
                "toolName": req.get("tool_name"),
                "toolUseId": req.get("tool_use_id"),
                "input": input,
                "suggestions": req.get("permission_suggestions"),
            }))
            .await;
    }

    /// Answer a pending permission prompt (from a client `permission_response`).
    /// `allow` runs the tool (with `updated_input`, else the original input, as
    /// `updatedInput`); otherwise it's denied with `message`. Returns false if no
    /// turn is awaiting input.
    pub async fn respond_permission(
        &self,
        request_id: &str,
        allow: bool,
        message: Option<String>,
        updated_input: Option<Value>,
    ) -> bool {
        let cached = self.pending_perms.lock().await.remove(request_id);
        let tx = { self.stdin_tx.lock().await.clone() };
        let Some(tx) = tx else {
            return false;
        };
        let inner = if allow {
            serde_json::json!({
                "behavior": "allow",
                "updatedInput": updated_input.or(cached).unwrap_or_else(|| serde_json::json!({})),
            })
        } else {
            serde_json::json!({
                "behavior": "deny",
                "message": message.unwrap_or_else(|| "Denied by user".to_string()),
            })
        };
        let line = serde_json::json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id, "response": inner }
        })
        .to_string();
        tx.send(line).await.is_ok()
    }

    /// Best-effort interrupt of the current turn: kills the live child if any.
    pub async fn stop(&self) {
        // Kill the live child in place; the owning reader still reaps it.
        let cell = { self.child.lock().await.clone() };
        if let Some(cell) = cell {
            if let Some(c) = cell.lock().await.as_mut() {
                let _ = c.start_kill();
            }
        }
    }

    // stream-json: line-delimited events on stdout
    async fn read_stream(
        &self,
        cell: Arc<tokio::sync::Mutex<Option<Child>>>,
        tx: &mpsc::Sender<Value>,
    ) -> usize {
        // Take the stdout/stderr pipes out of the child in place; the Child
        // itself stays in the cell so Stop can still start_kill it.
        let (stdout, stderr) = {
            let mut guard = cell.lock().await;
            let child = match guard.as_mut() {
                Some(c) => c,
                None => return 0,
            };
            (child.stdout.take().unwrap(), child.stderr.take().unwrap())
        };
        let sid = self.id.clone();
        let tx2 = tx.clone();
        tokio::spawn(forward_stderr(stderr, tx2, sid));

        let mut produced = 0usize;
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.is_empty() {
                continue;
            }
            if let Ok(evt) = serde_json::from_str::<Value>(&line) {
                // Permission prompt over the stdio control protocol: surface it to
                // the client as a `permission_request` and keep reading; the
                // decision arrives later via `respond_permission` (written to stdin).
                if evt.get("type").and_then(|v| v.as_str()) == Some("control_request")
                    && evt.pointer("/request/subtype").and_then(|v| v.as_str())
                        == Some("can_use_tool")
                {
                    self.emit_permission_request(&evt, tx).await;
                    continue;
                }
                let is_result = evt.get("type").and_then(|v| v.as_str()) == Some("result");
                if self.ingest(&evt, tx).await {
                    produced += 1;
                }
                // Turn over: close stdin (held open for approvals) so claude sees
                // EOF and exits, ending this read loop.
                if is_result {
                    *self.stdin_tx.lock().await = None;
                }
            } else {
                let _ = tx
                    .send(serde_json::json!({
                        "type": "stderr", "sessionId": self.id, "text": line
                    }))
                    .await;
            }
        }
        // Reap the child once stdout hits EOF.
        if let Some(mut c) = cell.lock().await.take() {
            let _ = c.wait().await;
        }
        produced
    }

    // buffered json: collect stdout, parse as array at the end
    async fn read_json(
        &self,
        cell: Arc<tokio::sync::Mutex<Option<Child>>>,
        tx: &mpsc::Sender<Value>,
    ) -> usize {
        let (mut stdout, stderr) = {
            let mut guard = cell.lock().await;
            let child = match guard.as_mut() {
                Some(c) => c,
                None => return 0,
            };
            (child.stdout.take().unwrap(), child.stderr.take().unwrap())
        };
        let tx2 = tx.clone();
        let sid = self.id.clone();
        tokio::spawn(forward_stderr(stderr, tx2, sid));

        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf).await;
        if let Some(mut c) = cell.lock().await.take() {
            let _ = c.wait().await;
        }

        // The user echo is broadcast once by the manager on send (so every
        // device sees it); the json path no longer re-emits it here.

        // Count only substantive ingest events (assistant/result/progress).
        // Do not pre-seed with 1: the echoed user turn is not model output, and
        // pre-seeding masks total failures (empty/crashed subprocess) as success.
        let mut produced = 0usize;
        if let Ok(arr) = serde_json::from_str::<Vec<Value>>(&buf) {
            for evt in arr {
                if self.ingest(&evt, tx).await {
                    produced += 1;
                }
            }
        } else if !buf.trim().is_empty() {
            let _ = tx
                .send(serde_json::json!({
                    "type": "stderr", "sessionId": self.id, "text": buf.chars().rev().take(500).collect::<String>()
                }))
                .await;
        }
        produced
    }

    async fn ingest(&self, evt: &Value, tx: &mpsc::Sender<Value>) -> bool {
        // capture the persistent Claude Code session id from authoritative sources
        if let Some(sid) = evt.get("session_id").and_then(|v| v.as_str()) {
            let is_init = evt.get("type").and_then(|v| v.as_str()) == Some("system")
                && evt.get("subtype").and_then(|v| v.as_str()) == Some("init");
            let is_ok_result = evt.get("type").and_then(|v| v.as_str()) == Some("result")
                && evt.get("is_error").and_then(|v| v.as_bool()) != Some(true);
            if is_init || is_ok_result {
                *self.cc_session_id.lock().await = Some(sid.to_string());
            }
        }
        let produced = counts_as_produced(evt);
        // Log errored result frames explicitly — these are the silent-failure cases.
        if !produced && evt.get("type").and_then(|v| v.as_str()) == Some("result") {
            tracing::warn!(
                session_id = %self.id,
                subtype = ?evt.get("subtype").and_then(|v| v.as_str()),
                errors = ?evt.get("errors"),
                "errored result frame (turn will not count as produced)"
            );
        }
        let _ = tx.send(evt.clone()).await;
        produced
    }
}

/// Whether a forwarded event counts as substantive model output for the
/// stream→json fallback / failure detection. An *errored* `result` frame
/// (e.g. `subtype:"error_during_execution"`, or `is_error:true` such as
/// "No conversation found with session ID") is NOT output: counting it would
/// suppress the bridge_error fallback and leave the client with a silent,
/// response-less turn.
fn counts_as_produced(evt: &Value) -> bool {
    match evt.get("type").and_then(|v| v.as_str()) {
        Some("assistant") | Some("user") | Some("progress") => true,
        Some("result") => evt.get("is_error").and_then(|v| v.as_bool()) != Some(true),
        _ => false,
    }
}

async fn forward_stderr<R: AsyncReadExt + Unpin>(
    mut stderr: R,
    tx: mpsc::Sender<Value>,
    sid: String,
) {
    let mut lines = BufReader::new(&mut stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let _ = tx
            .send(serde_json::json!({
                "type": "stderr", "sessionId": sid, "text": line
            }))
            .await;
    }
}

/// Owns a streaming turn's stdin: writes the user message, then forwards any
/// control-response lines pushed on the channel until it closes (turn end) — at
/// which point stdin drops and claude sees EOF.
async fn stdin_writer(mut stdin: ChildStdin, user_line: String, mut rx: mpsc::Receiver<String>) {
    if stdin
        .write_all(format!("{}\n", user_line).as_bytes())
        .await
        .is_err()
    {
        return;
    }
    let _ = stdin.flush().await;
    while let Some(line) = rx.recv().await {
        if stdin
            .write_all(format!("{}\n", line).as_bytes())
            .await
            .is_err()
        {
            break;
        }
        let _ = stdin.flush().await;
    }
}

pub fn new_session_id() -> String {
    Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn errored_result_does_not_count_as_produced() {
        // The exact frame seen for an unresumable session.
        let evt = json!({
            "type": "result",
            "subtype": "error_during_execution",
            "is_error": true,
            "errors": ["No conversation found with session ID: 5e993469"],
        });
        assert!(!counts_as_produced(&evt));
    }

    #[test]
    fn ok_result_and_assistant_count_as_produced() {
        assert!(counts_as_produced(
            &json!({"type":"result","is_error":false})
        ));
        assert!(counts_as_produced(&json!({"type":"result"})));
        assert!(counts_as_produced(&json!({"type":"assistant"})));
        assert!(counts_as_produced(&json!({"type":"user"})));
    }

    #[test]
    fn non_substantive_frames_do_not_count() {
        assert!(!counts_as_produced(
            &json!({"type":"system","subtype":"init"})
        ));
        assert!(!counts_as_produced(&json!({"type":"stderr","text":"x"})));
    }

    fn test_session(mode: Option<&str>) -> ClaudeSession {
        ClaudeSession::new(
            ClaudeBin(PathBuf::from("claude")),
            "id".into(),
            PathBuf::from("/tmp"),
            None,
            None,
            mode.map(|m| m.into()),
            None,
        )
    }

    #[test]
    fn permission_mode_round_trips_and_empty_clears() {
        let s = test_session(Some("default"));
        assert_eq!(s.permission_mode().as_deref(), Some("default"));
        s.set_permission_mode(Some("acceptEdits".into()));
        assert_eq!(s.permission_mode().as_deref(), Some("acceptEdits"));
        s.set_permission_mode(Some(String::new())); // empty selection clears
        assert_eq!(s.permission_mode(), None);
    }

    #[test]
    fn streaming_args_request_stdio_permission_prompt() {
        let s = test_session(None);
        let streaming = s.base_args(true, &None);
        assert!(streaming
            .windows(2)
            .any(|w| w == ["--permission-prompt-tool".to_string(), "stdio".to_string()]));
        // The buffered fallback has no interactive control channel, so it must NOT
        // request the stdio prompt (the turn would hang waiting on closed stdin).
        let buffered = s.base_args(false, &None);
        assert!(!buffered.iter().any(|a| a == "--permission-prompt-tool"));
    }

    #[tokio::test]
    async fn respond_permission_without_active_turn_is_false() {
        // No streaming turn running → no stdin channel → the decision is a no-op.
        let s = test_session(None);
        assert!(!s.respond_permission("req-1", true, None, None).await);
    }
}

// ---- `claude agents --json` discovery ----
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedEntry {
    #[serde(default)]
    pub id: Option<String>,
    /// Full Claude Code session id (the transcript file is named after this).
    /// `claude agents --json` emits this as `sessionId`.
    #[serde(default, alias = "sessionId")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default, alias = "startedAt")]
    pub started_at: Option<u64>,
}

pub async fn list_managed(bin: &ClaudeBin) -> Result<Vec<ManagedEntry>> {
    // `claude agents` can hang indefinitely on some installs (waiting on a TTY
    // or a gateway). Bound it so server startup never blocks on discovery.
    let fut = async {
        let out = Command::new(&bin.0)
            .args(["agents", "--json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;
        let parsed: Vec<ManagedEntry> = serde_json::from_slice(&out.stdout).unwrap_or_default();
        Ok::<_, std::io::Error>(parsed)
    };
    match tokio::time::timeout(std::time::Duration::from_secs(5), fut).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(anyhow::anyhow!(e)),
        Err(_) => {
            tracing::warn!("`claude agents --json` timed out after 5s; skipping session discovery");
            Ok(Vec::new())
        }
    }
}
