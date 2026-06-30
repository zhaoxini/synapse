// Transcript backfill: read a persisted Claude Code session transcript from
// disk and return the conversational events (user / assistant / system-init)
// in order, shaped like the live stream-json events the client already
// ingests. Mirrors web/src/bridge/history.js.
//
// Claude Code stores transcripts at:
//   ~/.claude/projects/<cwd-with-slashes-as-dashes>/<sessionId>.jsonl
// Each line is one JSON event; we keep only message-bearing events so the
// backfilled transcript matches what a live session would have streamed.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

fn claude_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".claude"))
                .unwrap_or_else(|| PathBuf::from(".claude"))
        })
}

// Claude Code encodes cwd by replacing non-alnum with '-'.
fn encode_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn project_dir(cwd: &str) -> PathBuf {
    claude_dir().join("projects").join(encode_cwd(cwd))
}

pub fn transcript_path(cwd: &str, session_id: &str) -> PathBuf {
    project_dir(cwd).join(format!("{session_id}.jsonl"))
}

/// Read a transcript and return normalized conversational events, capped to the
/// most recent `limit` to bound payload size. Returns `found: false` if the
/// file is absent or unreadable.
pub async fn load_transcript(cwd: &str, session_id: &str, limit: usize) -> (Vec<Value>, bool) {
    let path = transcript_path(cwd, session_id);
    load_from(&path, session_id, limit).await
}

/// Normalize one raw transcript line into a client-facing event, or `None` if
/// the line isn't a message-bearing event the client renders. Tags with
/// `sessionId` and drops transcript-internal fields. Keeps `message.id` so the
/// live tailer's assistant frames reconcile against streamed ones on the
/// desktop client (the web client ignores it). Shared by history backfill and
/// the live transcript tailer so both apply identical filtering.
pub fn normalize_line(line: &str, session_id: &str) -> Option<Value> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let mut evt = serde_json::from_str::<Value>(line).ok()?;
    let ty = evt.get("type").and_then(|v| v.as_str()).unwrap_or("");
    // Keep only message-bearing events the client renders.
    if ty != "user" && ty != "assistant" && ty != "system" {
        return None;
    }
    if ty == "system" && evt.get("subtype").and_then(|v| v.as_str()) != Some("init") {
        return None;
    }
    if evt.get("isSidechain").and_then(|v| v.as_bool()) == Some(true) {
        return None;
    }
    if let Some(obj) = evt.as_object_mut() {
        obj.insert("sessionId".into(), Value::String(session_id.into()));
        if let Some(m) = obj.get_mut("message").and_then(|m| m.as_object_mut()) {
            m.remove("model");
        }
        obj.remove("parentUuid");
        obj.remove("promptId");
    }
    Some(evt)
}

async fn load_from(path: &Path, session_id: &str, limit: usize) -> (Vec<Value>, bool) {
    let data = match tokio::fs::read_to_string(path).await {
        Ok(d) => d,
        Err(_) => return (Vec::new(), false),
    };
    let mut events: Vec<Value> = data
        .lines()
        .filter_map(|line| normalize_line(line, session_id))
        .collect();
    let found = !events.is_empty();
    if events.len() > limit {
        events = events.split_off(events.len() - limit);
    }
    (events, found)
}

/// First user-typed message in a transcript, used as a session-list title so
/// the list shows what each session is about instead of a generic name. Reads
/// line-by-line and stops at the first real prompt (transcripts can be large).
/// Skips tool-result frames (no text) and system-injected envelopes (`<...>`).
pub async fn first_user_text(cwd: &str, session_id: &str) -> Option<String> {
    use tokio::io::AsyncBufReadExt;
    let file = tokio::fs::File::open(transcript_path(cwd, session_id))
        .await
        .ok()?;
    let mut lines = tokio::io::BufReader::new(file).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let evt: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if evt.get("type").and_then(|v| v.as_str()) != Some("user") {
            continue;
        }
        if evt.get("isSidechain").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        let content = evt.get("message").and_then(|m| m.get("content"));
        let text = match content {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(|b| {
                    if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                        b.get("text").and_then(|t| t.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
            _ => continue,
        };
        let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
        // ponytail: skip system-injected reminders / slash-command envelopes; the
        // first real prompt is the title. A prompt that's only an envelope yields
        // no title and the caller falls back to "New session".
        if t.is_empty() || t.starts_with('<') {
            continue;
        }
        // Strip command/hook boilerplate (/goal stop-hooks, continuation
        // summaries, caveats) BEFORE truncating, so the 60-char title reads as
        // the session's real intent. Pure boilerplate → skip to the next line.
        let t = clean_title(&t);
        if t.is_empty() {
            continue;
        }
        let head: String = t.chars().take(60).collect();
        return Some(if t.chars().count() > 60 {
            format!("{}…", head.trim_end())
        } else {
            head
        });
    }
    None
}

/// Strip command/hook boilerplate from a transcript line so the session title
/// reads as the real intent. Returns "" when the line is pure boilerplate (the
/// caller then falls through to the next user message).
fn clean_title(t: &str) -> String {
    // /goal stop-hook line: the meaningful bit is the quoted condition.
    if let Some(rest) = t.split("Stop hook is now active with condition: \"").nth(1) {
        return rest.split('"').next().unwrap_or(rest).trim().to_string();
    }
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("this session is being continued") {
        return "Continued session".to_string();
    }
    if lower.starts_with("caveat:") {
        // Drop the caveat sentence; keep whatever real text follows it.
        return t
            .split("explicitly requested.")
            .nth(1)
            .unwrap_or("")
            .trim()
            .to_string();
    }
    for cmd in ["/goal ", "/compact ", "/clear ", "/model ", "/ponytail "] {
        if let Some(rest) = t.strip_prefix(cmd) {
            return rest.trim().to_string();
        }
    }
    t.to_string()
}

/// Inline normalize helper used when building a client-facing history payload.
pub fn _shape_session(_obj: &Map<String, Value>) -> Value {
    Value::Null
}
