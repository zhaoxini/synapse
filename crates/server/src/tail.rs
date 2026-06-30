// Live transcript tailer: mirrors sessions driven OUTSIDE Synapse (a native
// Claude Code run in a terminal/IDE) to every connected client in near-real
// time. Such turns never flow through the manager's runner, so the only
// observable is Claude Code's append-only `.jsonl` transcript. We poll each
// session's transcript, broadcast newly-appended message lines, and synthesize
// turn_started/turn_stopped from the entries so mobile shows the "replying"
// status.
//
// Granularity is message-level (the transcript holds whole messages, not token
// deltas) — token streaming only exists for Synapse-driven turns. To avoid
// double-broadcasting those, the tailer stays silent while a session's runner
// turn is active (state == Busy) and for a short cooldown after, only advancing
// its byte offset.

use crate::claude::SessionState;
use crate::history;
use crate::manager::SessionManager;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::SeekFrom;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

const POLL: Duration = Duration::from_millis(800);
// Don't broadcast transcript lines for this long after a Synapse runner turn:
// the runner already streamed them, and the final frames land on disk slightly
// after state flips back to Idle.
const RUNNER_COOLDOWN: Duration = Duration::from_secs(2);
// Clear a synthesized busy state if the transcript goes quiet this long (a
// terminal turn that crashed or whose final stop_reason we never saw).
const BUSY_TIMEOUT: Duration = Duration::from_secs(90);

struct Tail {
    offset: u64,
    busy: bool,
    last_activity: Instant,
    last_busy_seen: Option<Instant>,
}

/// Spawn the background tailer. Picks up sessions as the manager learns of them
/// (it re-snapshots every poll), so attaching/creating sessions needs no signal.
pub fn spawn(manager: Arc<SessionManager>) {
    tokio::spawn(async move {
        let mut tails: HashMap<String, Tail> = HashMap::new();
        loop {
            tokio::time::sleep(POLL).await;
            for (id, cwd, cc_sid, state) in manager.tail_snapshot().await {
                let Some(sid) = cc_sid else { continue };
                let path = history::transcript_path(&cwd, &sid);
                let size = match tokio::fs::metadata(&path).await {
                    Ok(m) => m.len(),
                    Err(_) => continue, // no transcript yet
                };
                // First sight: start at EOF so we don't replay history (clients
                // backfill via op:history).
                let t = tails.entry(id.clone()).or_insert(Tail {
                    offset: size,
                    busy: false,
                    last_activity: Instant::now(),
                    last_busy_seen: None,
                });

                let synapse_busy = state == SessionState::Busy;
                if synapse_busy {
                    t.last_busy_seen = Some(Instant::now());
                }
                let in_cooldown = t
                    .last_busy_seen
                    .map(|s| s.elapsed() < RUNNER_COOLDOWN)
                    .unwrap_or(false);

                if size < t.offset {
                    t.offset = size; // file rotated/truncated
                }
                if size > t.offset {
                    if let Some((consumed, lines)) = read_tail(&path, t.offset, size).await {
                        t.offset += consumed;
                        if !synapse_busy && !in_cooldown {
                            broadcast_lines(&manager, &id, &lines, t).await;
                        }
                    }
                }

                if t.busy && t.last_activity.elapsed() > BUSY_TIMEOUT {
                    t.busy = false;
                    manager.broadcast(turn_evt(&id, false)).await;
                }
            }
        }
    });
}

/// Parse + broadcast each new message line, then emit a turn_started/turn_stopped
/// transition derived from the last status-bearing entry in the batch.
async fn broadcast_lines(manager: &SessionManager, id: &str, lines: &[String], t: &mut Tail) {
    let mut last_status: Option<bool> = None;
    for line in lines {
        if let Some(evt) = history::normalize_line(line, id) {
            if let Some(b) = status_of(&evt) {
                last_status = Some(b);
            }
            manager.broadcast(evt).await;
        }
    }
    if let Some(busy) = last_status {
        t.last_activity = Instant::now();
        if busy != t.busy {
            t.busy = busy;
            manager.broadcast(turn_evt(id, busy)).await;
        }
    }
}

/// Whether an entry implies a turn is in progress (busy) or done (idle).
/// `None` for entries with no status signal (e.g. system/init).
fn status_of(evt: &Value) -> Option<bool> {
    match evt.get("type").and_then(|v| v.as_str()) {
        // A user prompt or a tool_result both mean the model still owes a reply.
        Some("user") => Some(true),
        // Assistant is "still working" while tools run (stop_reason tool_use) or
        // before a stop_reason is recorded; any terminal stop_reason ends the turn.
        Some("assistant") => {
            let sr = evt.pointer("/message/stop_reason").and_then(|v| v.as_str());
            Some(sr.is_none() || sr == Some("tool_use"))
        }
        _ => None,
    }
}

fn turn_evt(id: &str, busy: bool) -> Value {
    json!({
        "type": "system",
        "subtype": if busy { "turn_started" } else { "turn_stopped" },
        "sessionId": id,
    })
}

/// Read `[offset, size)` and return (bytes consumed up to the last complete
/// line, those lines). Returns `None` if there is no complete line yet (a
/// partial write); the bytes stay buffered for the next poll.
async fn read_tail(path: &std::path::Path, offset: u64, size: u64) -> Option<(u64, Vec<String>)> {
    let mut f = tokio::fs::File::open(path).await.ok()?;
    f.seek(SeekFrom::Start(offset)).await.ok()?;
    let mut buf = vec![0u8; (size - offset) as usize];
    f.read_exact(&mut buf).await.ok()?;
    let last_nl = buf.iter().rposition(|&b| b == b'\n')?;
    let text = String::from_utf8_lossy(&buf[..=last_nl]);
    let lines = text.lines().map(|s| s.to_string()).collect();
    Some((last_nl as u64 + 1, lines))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_user_is_busy() {
        let e = json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"hi"}]}});
        assert_eq!(status_of(&e), Some(true));
    }

    #[test]
    fn status_assistant_tool_use_is_busy_end_turn_is_idle() {
        let busy = json!({"type":"assistant","message":{"stop_reason":"tool_use"}});
        let idle = json!({"type":"assistant","message":{"stop_reason":"end_turn"}});
        assert_eq!(status_of(&busy), Some(true));
        assert_eq!(status_of(&idle), Some(false));
    }

    #[test]
    fn status_non_message_is_none() {
        assert_eq!(status_of(&json!({"type":"system","subtype":"init"})), None);
    }
}
