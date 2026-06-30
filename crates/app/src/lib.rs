//! Synapse app library: shared app logic plus the iOS entry point.
//! The desktop binary (`src/main.rs`) calls [`run_app`].

slint::include_modules!();

mod net;
pub mod web;

use net::{NetCmd, NetHandle};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

fn model_rc<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

// Per-turn streaming scratch held on the UI thread. The net thread delivers
// each forwarded event through `invoke_from_event_loop` -> `handle_event`, so
// `STREAM` is only ever touched from the UI thread. `dirty` signals the flush
// timer that new streamed content needs rendering.
thread_local! {
    static STREAM: std::cell::RefCell<StreamState> = std::cell::RefCell::new(StreamState::new());
}

/// Upsert the current streamed message's blocks into the messages model,
/// incrementally (row-granular signals, not a full rebuild). Returns true if
/// it touched the model.
fn flush_stream(app: &App) -> bool {
    // Snapshot the streamed blocks out of STREAM first so we release the borrow
    // before touching the Slint model (which may fire callbacks).
    let (mid, blocks): (String, Vec<MsgBlock>) = STREAM.with(|cell| {
        let st = cell.borrow();
        if st.message_id.is_empty() || st.block_count() == 0 {
            return (String::new(), Vec::new());
        }
        let blocks = (0..st.block_count())
            .filter_map(|p| st.block_at(p).cloned())
            .collect();
        (st.message_id.clone(), blocks)
    });
    if mid.is_empty() || blocks.is_empty() {
        return false;
    }
    let model = app.get_messages();
    let Some(vm) = model.as_any().downcast_ref::<VecModel<MsgBlock>>() else {
        return false;
    };
    // First row index belonging to the streamed message (keyed by messageId).
    let start = (0..vm.row_count()).find(|&r| {
        vm.row_data(r)
            .map(|b| b.messageId == mid.as_str())
            .unwrap_or(false)
    });
    match start {
        None => {
            for b in blocks {
                vm.push(b);
            }
        }
        Some(start) => {
            for (p, block) in blocks.into_iter().enumerate() {
                let row = start + p;
                if row < vm.row_count() {
                    vm.set_row_data(row, block);
                } else {
                    vm.push(block);
                }
            }
        }
    }
    true
}

/// Replace the streamed message (keyed by `message_id`) with the authoritative
/// final blocks from the `assistant` frame, splitting markdown into text/code
/// rows. De-dups the live-streamed rows so the turn's answer isn't duplicated.
fn replace_streamed_message(app: &App, message_id: &str, blocks: &[MsgBlock]) {
    let model = app.get_messages();
    let Some(vm) = model.as_any().downcast_ref::<VecModel<MsgBlock>>() else {
        return;
    };
    // Remove all rows belonging to the streamed message, remembering the slot.
    let mut first_removed: Option<usize> = None;
    let mut i = 0;
    while i < vm.row_count() {
        let mine = vm
            .row_data(i)
            .map(|b| b.messageId == message_id)
            .unwrap_or(false);
        if mine {
            if first_removed.is_none() {
                first_removed = Some(i);
            }
            vm.remove(i);
        } else {
            i += 1;
        }
    }
    let insert_at = first_removed.unwrap_or_else(|| vm.row_count());
    for (p, b) in blocks.iter().enumerate() {
        vm.insert(insert_at + p, b.clone());
    }
}

/// Current local time as a short "HH:MM" string for message timestamps.
/// Uses a simple manual breakdown of the Unix epoch seconds (from
/// `SystemTime`) plus the local offset from `chrono`-free arithmetic is not
/// available without a crate, so we format UTC and accept minor drift on the
/// display clock — timestamps are a secondary affordance, not authoritative.
fn now_time() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Use libc::localtime_r for correct local-time display (respects $TZ /
    // system timezone). Falls back to UTC arithmetic on non-unix platforms.
    #[cfg(unix)]
    {
        unsafe {
            let mut tm: libc_tm = std::mem::zeroed();
            let t = secs as LibcTimeT;
            if libc_localtime_r(&t, &mut tm).is_null() {
                return utc_hhmm(secs);
            }
            format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
        }
    }
    #[cfg(not(unix))]
    {
        utc_hhmm(secs)
    }
}

fn utc_hhmm(secs: u64) -> String {
    let rem = secs % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    format!("{:02}:{:02}", h, m)
}

// Minimal FFI for localtime_r so we don't pull in the `libc` crate.
#[cfg(unix)]
#[repr(C)]
struct libc_tm {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,
    tm_year: i32,
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
    tm_gmtoff: i64,
    tm_zone: *const i8,
}

#[cfg(unix)]
type LibcTimeT = i64;

#[cfg(unix)]
extern "C" {
    fn localtime_r(time: *const LibcTimeT, result: *mut libc_tm) -> *mut libc_tm;
}

#[cfg(unix)]
unsafe fn libc_localtime_r(time: *const LibcTimeT, result: *mut libc_tm) -> *mut libc_tm {
    localtime_r(time, result)
}

pub fn run_app() -> anyhow::Result<()> {
    let _ = env_logger::try_init();
    // Ensure a rustls crypto provider is installed before any TLS handshake.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let app = App::new()?;

    // On iOS, expose a weak handle so the UIKit keyboard observer can drive
    // `keyboardInset`. (Desktop builds have no such observer; this is a no-op.)
    #[cfg(target_os = "ios")]
    ios_install_app(&app);

    // Spawn the background network thread. All WebSocket I/O happens there on
    // its own tokio runtime; the main thread (this one) only runs the Slint
    // event loop and pushes commands through this handle.
    let net: NetHandle = net::spawn_net_thread(app.as_weak());

    // --- pair via QR / link ---
    // Parses a synapse://host:port?token=T&tls=N link, fills the pairing
    // fields, and connects — the path taken after scanning the server's QR.
    {
        let weak = app.as_weak();
        let net = net.clone();
        app.on_pairClicked(move |link| {
            let net = net.clone();
            let raw = link.trim().to_string();
            // Accept three input shapes:
            //   1. full synapse://host:port?token=..&tls=.. link (from QR)
            //   2. bare host:port?token=.. authority+query
            //   3. a bare pairing CODE -> combined with the last-known
            //      host/port/tls already in the manual fields.
            let parsed = net::parse_pair_link(&raw).or_else(|| {
                if raw.is_empty() || raw.contains(' ') {
                    return None;
                }
                let looks_like_code = raw
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
                if !looks_like_code {
                    return None;
                }
                let app = weak.upgrade()?;
                let host = app.get_pairingHost().to_string();
                let port = app.get_pairingPort().to_string();
                let tls = app.get_pairingTls();
                if host.is_empty() || port.is_empty() {
                    return None;
                }
                Some(net::ParsedPair {
                    host,
                    port,
                    token: raw.clone(),
                    tls,
                    path: String::new(),
                })
            });
            let parsed = match parsed {
                Some(p) => p,
                None => {
                    if let Some(app) = weak.upgrade() {
                        app.set_pairingError(
                            "Enter the pairing code from your server, or paste the synapse:// link.".into(),
                        );
                    }
                    return;
                }
            };
            if let Some(app) = weak.upgrade() {
                app.set_pairingHost(parsed.host.clone().into());
                app.set_pairingPort(parsed.port.clone().into());
                app.set_pairingToken(parsed.token.clone().into());
                app.set_pairingTls(parsed.tls);
                app.set_pairingError("".into());
                app.set_pairLinkText("".into());
                app.set_showPairSheet(false);
                app.set_connecting(true);
            }
            // Hand off to the background network thread.
            net.send(NetCmd::Connect {
                host: parsed.host,
                port: parsed.port,
                token: parsed.token,
                tls: parsed.tls,
                path: parsed.path,
            });
        });
    }

    // --- connect ---
    {
        let weak = app.as_weak();
        let net = net.clone();
        app.on_connectClicked(move || {
            let net = net.clone();
            let app = weak.unwrap();
            let host = app.get_pairingHost().to_string();
            let port = app.get_pairingPort().to_string();
            let token = app.get_pairingToken().to_string();
            let tls = app.get_pairingTls();
            app.set_connecting(true);
            net.send(NetCmd::Connect {
                host,
                port,
                token,
                tls,
                path: String::new(),
            });
        });
    }

    {
        let weak = app.as_weak();
        app.on_toggleTool(move |idx| {
            let app = weak.unwrap();
            let model = app.get_messages();
            let Some(vm) = model.as_any().downcast_ref::<VecModel<MsgBlock>>() else {
                return;
            };
            let row = idx as usize;
            if let Some(mut m) = vm.row_data(row) {
                m.expanded = !m.expanded;
                vm.set_row_data(row, m);
            }
        });
    }

    // --- send message ---
    {
        let weak = app.as_weak();
        let net = net.clone();
        app.on_sendClicked(move |text| {
            let net = net.clone();
            let app = weak.unwrap();
            let sid = app.get_activeSessionId().to_string();
            if sid.is_empty() {
                return;
            }
            // No optimistic echo: the server broadcasts the user message to every
            // device viewing this session, and we render it from that broadcast
            // (append_live_user) — so all devices show an identical transcript.
            app.set_composerText("".into());
            app.set_busy(true);
            let msg =
                serde_json::json!({ "op": "send", "sessionId": sid, "content": text.as_str() });
            net.send(NetCmd::Send(msg.to_string()));
        });
    }

    // --- stop current turn ---
    {
        let weak = app.as_weak();
        let net = net.clone();
        app.on_stopClicked(move || {
            let sid = weak.unwrap().get_activeSessionId().to_string();
            if sid.is_empty() {
                return;
            }
            let msg = serde_json::json!({ "op": "stop", "sessionId": sid });
            net.send(NetCmd::Send(msg.to_string()));
        });
    }

    // --- new session ---
    {
        let weak = app.as_weak();
        let net = net.clone();
        app.on_newSessionClicked(move || {
            let net = net.clone();
            let weak = weak.clone();
            let msg = serde_json::json!({ "op": "create", "opts": { "name": "New session" } });
            net.send(NetCmd::Send(msg.to_string()));
            if let Some(app) = weak.upgrade() {
                app.set_drawerOpen(false);
            }
        });
    }

    // --- drawer / select / refresh ---
    {
        let weak = app.as_weak();
        app.on_toggleDrawer(move || {
            let app = weak.unwrap();
            app.set_drawerOpen(!app.get_drawerOpen());
        });
    }
    {
        let weak = app.as_weak();
        let net = net.clone();
        app.on_selectSession(move |sid| {
            {
                let app = weak.unwrap();
                app.set_activeSessionId(sid.clone());
                app.set_drawerOpen(false);
                // clear while we request the backfilled transcript
                app.set_messages(ModelRc::new(VecModel::default()));
                // drop any in-flight streaming scratch from the previous session
                STREAM.with(|cell| cell.borrow_mut().reset());
                // update the header to reflect the newly-selected session
                app.set_busy(false); // default; overridden below if the session is busy
                let all = app.get_allSessions();
                if let Some(s) = all.iter().find(|s| s.id == sid.as_str()) {
                    app.set_activeSessionName(s.name.clone());
                    let sub = if s.model.is_empty() {
                        short_basename(&s.cwd)
                    } else {
                        format!("{} · {}", s.model, short_basename(&s.cwd))
                    };
                    app.set_activeSessionSub(sub.into());
                    app.set_activeState(s.state.clone());
                    // Seed busy from the session's current state so opening a
                    // session whose turn is already running (started on another
                    // device) shows the replying status at once; the live
                    // turn_stopped clears it.
                    app.set_busy(s.state == "busy");
                }
            }
            let msg =
                serde_json::json!({ "op": "history", "sessionId": sid.to_string(), "limit": 400 });
            net.send(NetCmd::Send(msg.to_string()));
        });
    }
    {
        let net = net.clone();
        app.on_refreshClicked(move || {
            let net = net.clone();
            let msg = serde_json::json!({ "op": "refresh" });
            net.send(NetCmd::Send(msg.to_string()));
        });
    }

    // --- suggestion chip tap: fill the composer (user can edit before send) ---
    {
        let weak = app.as_weak();
        app.on_suggestionClicked(move |prompt| {
            if let Some(app) = weak.upgrade() {
                app.set_composerText(prompt);
            }
        });
    }

    // --- copy code block to clipboard (desktop: arboard). Flashes "Copied".
    {
        let weak = app.as_weak();
        app.on_copyText(move |idx, text| {
            #[cfg(not(target_os = "android"))]
            {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    let _ = cb.set_text(text.to_string());
                }
            }
            if let Some(app) = weak.upgrade() {
                app.set_copiedIndex(idx);
                let weak2 = app.as_weak();
                slint::Timer::default().start(
                    slint::TimerMode::SingleShot,
                    std::time::Duration::from_millis(1400),
                    move || {
                        if let Some(app) = weak2.upgrade() {
                            // only clear if still showing this one (avoids clobbering a newer copy)
                            if app.get_copiedIndex() == idx {
                                app.set_copiedIndex(-1);
                            }
                        }
                    },
                );
            }
        });
    }

    // --- drawer search: recompute the visible session list from allSessions ---
    {
        let weak = app.as_weak();
        app.on_drawerFilterChanged(move |_filter| {
            let app = match weak.upgrade() {
                Some(a) => a,
                None => return,
            };
            let all = app.get_allSessions();
            let filter = app.get_drawerFilter().to_lowercase();
            let filtered: Vec<SessionInfo> = if filter.trim().is_empty() {
                all.iter()
                    .map(|si| SessionInfo {
                        id: si.id.clone(),
                        name: si.name.clone(),
                        cwd: si.cwd.clone(),
                        model: si.model.clone(),
                        state: si.state.clone(),
                        attached: si.attached,
                    })
                    .collect()
            } else {
                all.iter()
                    .filter(|si| {
                        si.name.to_lowercase().contains(&filter)
                            || si.cwd.to_lowercase().contains(&filter)
                    })
                    .map(|si| SessionInfo {
                        id: si.id.clone(),
                        name: si.name.clone(),
                        cwd: si.cwd.clone(),
                        model: si.model.clone(),
                        state: si.state.clone(),
                        attached: si.attached,
                    })
                    .collect()
            };
            app.set_sessions(model_rc(filtered));
        });
    }

    // --- paste pairing link from clipboard (scan-to-connect flow) ---
    // On mobile, the user scans the server's QR with the system camera app,
    // which copies the synapse:// link to the clipboard. Tapping "Paste link
    // from clipboard" pulls it into the input field.
    {
        let weak = app.as_weak();
        app.on_pasteClicked(move || {
            #[cfg(not(target_os = "android"))]
            {
                let text = arboard::Clipboard::new()
                    .ok()
                    .and_then(|mut cb| cb.get_text().ok())
                    .unwrap_or_default();
                if let Some(app) = weak.upgrade() {
                    let t = text.trim().to_string();
                    if !t.is_empty() {
                        app.set_pairLinkText(t.into());
                        app.set_pairingError("".into());
                    } else {
                        app.set_pairingError("Clipboard is empty. Scan the QR first.".into());
                    }
                }
            }
        });
    }

    // --- clipboard watcher: surface a "Paste link" affordance when the
    //     clipboard looks like a synapse:// pairing link. Polls every 1.2s
    //     while we're on the pairing screen, using a Slint repeating timer
    //     (runs on the main thread, no tokio needed). Desktop-only (arboard);
    //     on Android the native paste affordance is used instead. ---
    {
        let weak = app.as_weak();
        slint::Timer::default().start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(1200),
            move || {
                let Some(app) = weak.upgrade() else { return };
                if app.get_view().as_str() == "pairing" && app.get_pairLinkText().is_empty() {
                    #[cfg(not(target_os = "android"))]
                    {
                        let looks_like_link = arboard::Clipboard::new()
                            .ok()
                            .and_then(|mut cb| cb.get_text().ok())
                            .map(|t| {
                                let t = t.trim();
                                t.starts_with("synapse://")
                                    || (t.starts_with("http") && t.contains("token="))
                                    || (t.contains("://") && t.contains("token="))
                            })
                            .unwrap_or(false);
                        app.set_clipboardHasLink(looks_like_link);
                    }
                } else {
                    app.set_clipboardHasLink(false);
                }
            },
        );
    }

    // --- pulse timer: toggle `pulse` every 700ms so the typing dots breathe ---
    {
        let weak = app.as_weak();
        slint::Timer::default().start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(700),
            move || {
                let Some(app) = weak.upgrade() else { return };
                if app.get_busy() {
                    app.set_pulse(!app.get_pulse());
                }
            },
        );
    }

    // --- stream render timer: flush streamed blocks ~30x/sec. A turn can emit
    //     hundreds of content_block_delta events; parsing happens per-event but
    //     the model is only touched here, once per frame, when state is dirty. ---
    {
        let weak = app.as_weak();
        slint::Timer::default().start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(33),
            move || {
                let dirty = STREAM.with(|cell| {
                    let mut st = cell.borrow_mut();
                    let d = st.dirty;
                    st.dirty = false;
                    d
                });
                if dirty {
                    if let Some(app) = weak.upgrade() {
                        flush_stream(&app);
                    }
                }
            },
        );
    }

    // --- debug auto-connect (skips pairing screen for testing) ---
    // SYNAPSE_HOST=127.0.0.1 SYNAPSE_PORT=4173 SYNAPSE_TOKEN=CODE ./target/debug/synapse-app
    match (
        std::env::var("SYNAPSE_HOST"),
        std::env::var("SYNAPSE_PORT"),
        std::env::var("SYNAPSE_TOKEN"),
    ) {
        (Ok(host), Ok(port), Ok(token)) => {
            let tls = std::env::var("SYNAPSE_TLS")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            eprintln!("[SYNAPSE] auto-connect -> {host}:{port} tls={tls}");
            if let Some(app) = app.as_weak().upgrade() {
                app.set_connecting(true);
            }
            net.send(NetCmd::Connect {
                host,
                port,
                token,
                tls,
                path: String::new(),
            });
        }
        _ => {
            eprintln!("[SYNAPSE] no auto-connect env vars; staying on pairing screen");
        }
    }

    app.run()?;
    Ok(())
}

fn handle_event(app: &App, msg: serde_json::Value) {
    let ty = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match ty {
        "hello" => {
            let sessions = parse_sessions(msg.get("sessions"));
            apply_sessions(app, sessions);
            auto_select_if_needed(app);
        }
        "sessions" => {
            let sessions = parse_sessions(msg.get("sessions"));
            apply_sessions(app, sessions);
            auto_select_if_needed(app);
        }
        "created" => {
            if let Some(s) = msg.get("session") {
                let id = s
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = s
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Session")
                    .to_string();
                let model = s
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                app.set_activeSessionId(id.into());
                app.set_activeSessionName(name.into());
                let cwd = s.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
                let sub = if model.is_empty() {
                    short_basename(cwd)
                } else {
                    format!("{} · {}", model, short_basename(cwd))
                };
                app.set_activeSessionSub(sub.into());
                app.set_messages(ModelRc::new(VecModel::default()));
            }
        }
        "history" => {
            // backfilled transcript for the active session; rebuild message list
            let active = app.get_activeSessionId().to_string();
            let sid = msg
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if sid == active {
                if let Some(events) = msg.get("events").and_then(|v| v.as_array()) {
                    let mut msgs: Vec<MsgBlock> = Vec::new();
                    for evt in events {
                        ingest_event_into(&mut msgs, evt);
                    }
                    normalize_code_blocks(&mut msgs);
                    app.set_messages(model_rc(msgs));
                }
            }
        }
        "event" => {
            if let Some(evt) = msg.get("event") {
                dispatch_event(app, evt);
            }
        }
        "stderr" => {
            // The claude CLI writes diagnostics (auth errors, rate limits,
            // crashes) to stderr; surface them as a system message so the user
            // sees why a turn stalled instead of an infinite spinner.
            let sid = msg.get("sessionId").and_then(|v| v.as_str()).unwrap_or("");
            if sid.is_empty() || sid == app.get_activeSessionId().as_str() {
                if let Some(text) = msg.get("text").and_then(|v| v.as_str()) {
                    if !text.trim().is_empty() {
                        let mut msgs: Vec<MsgBlock> = app.get_messages().iter().collect();
                        push_system_error(&mut msgs, text);
                        app.set_messages(model_rc(msgs));
                    }
                }
            }
        }
        _ => {}
    }
}

fn apply_sessions(app: &App, sessions: Vec<SessionInfo>) {
    // Keep the unfiltered source list, then derive the visible list from the
    // current drawer filter (case-insensitive substring over name + cwd).
    app.set_allSessions(model_rc(sessions.clone()));
    let filter = app.get_drawerFilter().to_lowercase();
    let filtered: Vec<SessionInfo> = if filter.trim().is_empty() {
        sessions
    } else {
        sessions
            .into_iter()
            .filter(|si| {
                si.name.to_lowercase().contains(&filter) || si.cwd.to_lowercase().contains(&filter)
            })
            .collect()
    };
    app.set_sessions(model_rc(filtered));
}

/// Update one session's running state in the cached list from a turn_started/
/// turn_stopped/bridge_error broadcast for ANY session, so the drawer state and
/// busy-on-open reflect turns started on other devices. No-op if unchanged.
fn set_session_state(app: &App, sid: &str, st: &str) {
    if sid.is_empty() {
        return;
    }
    let mut sessions: Vec<SessionInfo> = app.get_allSessions().iter().collect();
    let mut changed = false;
    for s in sessions.iter_mut() {
        if s.id == sid {
            if s.state != st {
                s.state = st.into();
                changed = true;
            }
            break;
        }
    }
    if changed {
        apply_sessions(app, sessions);
    }
}

/// If no session is active yet and sessions are available, select the first
/// one in the header (name + subtitle + state) so the user has immediate
/// context instead of a blank chat. The transcript is fetched lazily on the
/// first interaction; this just populates the top bar.
fn auto_select_if_needed(app: &App) {
    if !app.get_activeSessionId().is_empty() {
        return;
    }
    let all = app.get_allSessions();
    if let Some(s) = all.iter().next() {
        app.set_activeSessionId(s.id.clone());
        app.set_activeSessionName(s.name.clone());
        let sub = if s.model.is_empty() {
            short_basename(&s.cwd)
        } else {
            format!("{} · {}", s.model, short_basename(&s.cwd))
        };
        app.set_activeSessionSub(sub.into());
        app.set_activeState(s.state.clone());
    }
}

fn parse_sessions(v: Option<&serde_json::Value>) -> Vec<SessionInfo> {
    let arr = match v.and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .map(|s| SessionInfo {
            id: s.get("id").and_then(|v| v.as_str()).unwrap_or("").into(),
            name: s
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Session")
                .into(),
            cwd: s.get("cwd").and_then(|v| v.as_str()).unwrap_or("").into(),
            model: s.get("model").and_then(|v| v.as_str()).unwrap_or("").into(),
            state: s
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("idle")
                .into(),
            attached: s.get("attached").and_then(|v| v.as_bool()).unwrap_or(false),
        })
        .collect()
}

/// Push the conversational content of one stream-json event into `msgs`.
/// Returns a TurnState transition (Some(true)=busy, Some(false)=idle) for the
/// caller to apply, since backfill should not flip the busy indicator.
fn ingest_event_into(msgs: &mut Vec<MsgBlock>, evt: &serde_json::Value) -> Option<bool> {
    let ty = evt.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let mut state: Option<bool> = None;
    match ty {
        "assistant" => {
            if let Some(content) = evt
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                for block in content {
                    let bt = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match bt {
                        "text" => {
                            let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            // Coalesce into the last assistant *text* segment of
                            // the current answer. Code blocks (already split out
                            // by normalize_code_blocks) are skipped so prose
                            // fragments keep accumulating on the trailing prose
                            // block; a subsequent normalize re-fences them.
                            let appended = if let Some(idx) = msgs
                                .iter()
                                .rposition(|m| m.role == "assistant" && m.kind == "text")
                            {
                                // Only coalesce if no tool block separates this
                                // text block from the end (i.e. it belongs to the
                                // same assistant turn).
                                let separated = msgs[idx + 1..]
                                    .iter()
                                    .any(|m| m.kind == "tool" || m.role == "user");
                                if separated {
                                    false
                                } else {
                                    let combined = format!("{}{}", msgs[idx].text, text);
                                    msgs[idx].text = combined.into();
                                    true
                                }
                            } else {
                                false
                            };
                            if !appended {
                                push_text(msgs, "assistant", text);
                            }
                        }
                        "tool_use" => {
                            let id = block
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = block
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("tool")
                                .to_string();
                            let arg_preview = tool_arg_preview(&name, block.get("input"));
                            // upsert by tool id
                            if let Some(existing) = msgs
                                .iter_mut()
                                .find(|m| m.kind == "tool" && m.toolId == id.as_str())
                            {
                                existing.toolName = name.into();
                                existing.toolStatus = "running".into();
                                existing.text = arg_preview.into();
                            } else {
                                msgs.push(MsgBlock {
                                    kind: "tool".into(),
                                    role: "assistant".into(),
                                    text: arg_preview.into(),
                                    toolName: name.into(),
                                    toolStatus: "running".into(),
                                    expanded: false,
                                    toolId: id.into(),
                                    codeLang: "".into(),
                                    time: "".into(),
                                    messageId: "".into(),
                                    blockIndex: 0,
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        "user" => {
            if let Some(content) = evt
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                let mut text_parts: Vec<String> = Vec::new();
                for block in content {
                    let bt = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match bt {
                        "tool_result" => {
                            let id = block
                                .get("tool_use_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let is_err = block
                                .get("is_error")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            let result_text = block
                                .get("content")
                                .map(|c| {
                                    if let Some(s) = c.as_str() {
                                        s.to_string()
                                    } else {
                                        c.to_string()
                                    }
                                })
                                .unwrap_or_default();
                            let result_preview = truncate(&result_text, 4000);
                            if let Some(existing) = msgs
                                .iter_mut()
                                .find(|m| m.kind == "tool" && m.toolId == id.as_str())
                            {
                                existing.toolStatus = if is_err { "error" } else { "done" }.into();
                                existing.text = SharedString::from(format!(
                                    "{}\n\n{}",
                                    existing.text, result_preview
                                ));
                            }
                        }
                        "text" => {
                            if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                                if !t.is_empty() {
                                    text_parts.push(t.to_string());
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if !text_parts.is_empty() {
                    let text = text_parts.join("\n");
                    let is_dup = msgs
                        .last()
                        .map(|m| m.role == "user" && m.text == text)
                        .unwrap_or(false);
                    if !is_dup {
                        push_text(msgs, "user", &text);
                    }
                }
            } else if let Some(content) = evt
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
            {
                if !content.is_empty() {
                    let is_dup = msgs
                        .last()
                        .map(|m| m.role == "user" && m.text == content)
                        .unwrap_or(false);
                    if !is_dup {
                        push_text(msgs, "user", content);
                    }
                }
            }
        }
        "result" => {
            state = Some(false);
        }
        "system" => {
            let sub = evt.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
            match sub {
                "turn_started" => state = Some(true),
                "turn_stopped" => state = Some(false),
                _ => {}
            }
        }
        _ => {}
    }
    state
}

fn push_text(msgs: &mut Vec<MsgBlock>, role: &str, text: &str) {
    msgs.push(MsgBlock {
        kind: "text".into(),
        role: role.into(),
        text: text.into(),
        toolName: "".into(),
        toolStatus: "".into(),
        expanded: false,
        toolId: "".into(),
        codeLang: "".into(),
        time: now_time().into(),
        messageId: "".into(),
        blockIndex: 0,
    });
}

/// Split assistant markdown into (kind, text, lang) segments, where `kind` is
/// either "text" or "code" and `lang` is the fence language tag (empty for
/// prose). Only fenced code blocks (``` ```) are extracted; inline formatting
/// is left as plain text because Slint dynamic text cannot style it.
/// Append a distinct system/error block so failures (stderr, bridge errors,
/// rate limits) are visible in the transcript instead of a silent spinner.
fn push_system_error(msgs: &mut Vec<MsgBlock>, text: &str) {
    // Collapse consecutive duplicate error lines to avoid spam on retry storms.
    let trimmed = text.trim();
    if msgs
        .last()
        .map(|m| m.kind == "error" && m.text.trim() == trimmed)
        .unwrap_or(false)
    {
        return;
    }
    msgs.push(MsgBlock {
        kind: "error".into(),
        role: "system".into(),
        text: trimmed.into(),
        toolName: "".into(),
        toolStatus: "".into(),
        expanded: false,
        toolId: "".into(),
        codeLang: "".into(),
        time: now_time().into(),
        messageId: "".into(),
        blockIndex: 0,
    });
}

fn split_markdown(md: &str) -> Vec<(&'static str, String, String)> {
    let mut out: Vec<(&'static str, String, String)> = Vec::new();
    let mut lines = md.split('\n').peekable();
    let mut prose = String::new();
    // flush accumulated prose as a single text segment
    let flush_prose = |prose: &mut String, out: &mut Vec<(&'static str, String, String)>| {
        if !prose.is_empty() {
            out.push(("text", std::mem::take(prose), String::new()));
        }
    };
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            flush_prose(&mut prose, &mut out);
            let lang = trimmed.strip_prefix("```").unwrap_or("").trim().to_string();
            let mut code: Vec<String> = Vec::new();
            let mut closed = false;
            for cline in lines.by_ref() {
                if cline.trim_start().starts_with("```") {
                    closed = true;
                    break;
                }
                code.push(cline.to_string());
            }
            // A code block is only emitted once the closing fence is seen.
            // While streaming, an unclosed fence stays in prose so the user
            // still sees partial output.
            if closed {
                out.push(("code", code.join("\n"), lang));
            } else {
                // unterminated fence during streaming: show raw so far
                let raw = format!("```{}\n{}", lang, code.join("\n"));
                out.push(("text", raw, String::new()));
            }
        } else {
            if !prose.is_empty() {
                prose.push('\n');
            }
            prose.push_str(line);
        }
    }
    flush_prose(&mut prose, &mut out);
    out
}

/// Re-expand any assistant text blocks into text/code segments based on their
/// current accumulated markdown. This runs after each event is ingested (and
/// after history backfill) so fenced code blocks render as ChatGPT-style dark
/// cards even though the content arrived as streaming text fragments. Tool
/// blocks and user blocks are left untouched.
fn normalize_code_blocks(msgs: &mut Vec<MsgBlock>) {
    let mut out: Vec<MsgBlock> = Vec::with_capacity(msgs.len());
    for m in msgs.iter() {
        if m.role == "assistant" && m.kind == "text" {
            for seg in split_markdown(m.text.as_str()) {
                if seg.1.is_empty() {
                    continue;
                }
                out.push(MsgBlock {
                    kind: seg.0.into(),
                    role: "assistant".into(),
                    text: seg.1.into(),
                    toolName: "".into(),
                    toolStatus: "".into(),
                    expanded: false,
                    toolId: "".into(),
                    codeLang: seg.2.into(),
                    time: "".into(),
                    messageId: "".into(),
                    blockIndex: 0,
                });
            }
        } else {
            out.push(m.clone());
        }
    }
    *msgs = out;
}

/// Build a short human-readable preview of a tool call input, mirroring the
/// Codex mobile / web prototype behavior (command, file_path, pattern, ...).
fn tool_arg_preview(name: &str, input: Option<&serde_json::Value>) -> String {
    let input = match input {
        Some(v) => v,
        None => return String::new(),
    };
    let low = name.to_ascii_lowercase();
    if low == "bash" {
        return input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
    }
    for key in &["file_path", "pattern", "path", "command", "query"] {
        if let Some(s) = input.get(*key).and_then(|v| v.as_str()) {
            return short_path(s);
        }
    }
    String::new()
}

fn short_path(p: &str) -> String {
    if p.is_empty() {
        return String::new();
    }
    let parts: Vec<&str> = p.split('/').collect();
    let len = parts.len();
    let start = len.saturating_sub(2);
    parts[start..].join("/")
}

/// Last path component of a working directory, e.g. "~/code/foo" -> "foo".
/// Used for the compact subtitle in the chat header.
fn short_basename(p: &str) -> String {
    if p.is_empty() {
        return String::new();
    }
    p.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(p)
        .to_string()
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    format!("{}\n…({} chars)", &s[..n], s.len())
}

/// Route one forwarded Claude Code event (the INNER `evt` from
/// `{"type":"event","event":<evt>}`) into the live transcript. This is the
/// streaming-aware path: `stream_event` deltas are parsed into `STREAM` and
/// flushed incrementally; the final `assistant` frame reconciles (de-dups).
fn dispatch_event(app: &App, evt: &serde_json::Value) {
    let sid = evt.get("sessionId").and_then(|v| v.as_str()).unwrap_or("");
    let ty = evt.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let active = app.get_activeSessionId();
    let is_active = sid.is_empty() || sid == active.as_str();

    // Turn status is tracked for EVERY session (not just the open one) so the
    // session list + busy-on-open reflect turns started on other devices. The
    // open session additionally flips the live busy/typing state.
    if ty == "system" {
        match evt.get("subtype").and_then(|v| v.as_str()).unwrap_or("") {
            "turn_started" => {
                set_session_state(app, sid, "busy");
                if is_active {
                    app.set_busy(true);
                    app.set_activeState("busy".into());
                }
            }
            "turn_stopped" => {
                set_session_state(app, sid, "idle");
                if is_active {
                    app.set_busy(false);
                    app.set_activeState("idle".into());
                    STREAM.with(|cell| cell.borrow_mut().reset());
                }
            }
            "bridge_error" => {
                set_session_state(app, sid, "error");
                if is_active {
                    let detail = evt
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("the Claude CLI produced no output");
                    push_live_system_error(app, &format!("Turn failed: {}", detail));
                    app.set_busy(false);
                    app.set_activeState("error".into());
                    app.set_toast("Turn failed — check CLI / API key".into());
                }
            }
            "api_retry" => {
                if is_active && app.get_busy() {
                    app.set_toast("Upstream rate-limited — retrying…".into());
                }
            }
            _ => {}
        }
        return;
    }

    // All other events are transcript output for one session — render only when
    // it's the session we're viewing.
    if !is_active {
        return;
    }
    match ty {
        "stream_event" => {
            // Parse into state + mark dirty; the render timer flushes ~30x/sec.
            STREAM.with(|cell| cell.borrow_mut().apply(evt));
        }
        "assistant" => {
            let mid = evt
                .pointer("/message/id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if mid.is_empty() {
                // Auth/quota error frames have no message.id; show their text as an error.
                if let Some(text) = evt
                    .pointer("/message/content/0/text")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                {
                    push_live_system_error(app, text);
                }
                return;
            }
            let blocks = assemble_assistant_blocks(&mid, evt);
            replace_streamed_message(app, &mid, &blocks);
            STREAM.with(|cell| {
                let st = cell.borrow();
                if st.message_id == mid {
                    drop(st);
                    cell.borrow_mut().reset();
                }
            });
        }
        "result" => {
            app.set_busy(false);
            app.set_activeState("idle".into());
            STREAM.with(|cell| cell.borrow_mut().reset());
        }
        "user" => {
            append_live_user(app, evt);
        }
        "stderr" => {
            if let Some(text) = evt.get("text").and_then(|v| v.as_str()) {
                if !text.trim().is_empty() {
                    push_live_system_error(app, text);
                }
            }
        }
        _ => {}
    }
}

/// Build the authoritative final block list for a top-level `assistant` frame,
/// splitting text markdown into text/code rows (so fenced code renders as cards).
fn assemble_assistant_blocks(message_id: &str, evt: &serde_json::Value) -> Vec<MsgBlock> {
    let mut out = Vec::new();
    let Some(content) = evt.pointer("/message/content").and_then(|c| c.as_array()) else {
        return out;
    };
    for (i, block) in content.iter().enumerate() {
        let bt = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match bt {
            "text" => {
                let t = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                for seg in split_markdown(t) {
                    out.push(MsgBlock {
                        kind: seg.0.into(),
                        role: "assistant".into(),
                        text: seg.1.into(),
                        toolName: "".into(),
                        toolStatus: "".into(),
                        expanded: false,
                        toolId: "".into(),
                        codeLang: seg.2.into(),
                        time: "".into(),
                        messageId: message_id.into(),
                        blockIndex: i as i32,
                    });
                }
            }
            "tool_use" => {
                let id = block
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool")
                    .to_string();
                out.push(MsgBlock {
                    kind: "tool".into(),
                    role: "assistant".into(),
                    text: tool_arg_preview(&name, block.get("input")).into(),
                    toolName: name.into(),
                    toolStatus: "running".into(),
                    expanded: false,
                    toolId: id.into(),
                    codeLang: "".into(),
                    time: "".into(),
                    messageId: message_id.into(),
                    blockIndex: i as i32,
                });
            }
            "thinking" => {
                out.push(MsgBlock {
                    kind: "thinking".into(),
                    role: "assistant".into(),
                    text: block
                        .get("thinking")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .into(),
                    toolName: "".into(),
                    toolStatus: "".into(),
                    expanded: false,
                    toolId: "".into(),
                    codeLang: "".into(),
                    time: "".into(),
                    messageId: message_id.into(),
                    blockIndex: i as i32,
                });
            }
            _ => {}
        }
    }
    out
}

/// Append a user-text block for a live `user` frame. The server is the single
/// source of the echo (broadcast once on send), so there is no optimistic local
/// echo to de-dup against — render it directly.
fn append_live_user(app: &App, evt: &serde_json::Value) {
    let text = evt
        .pointer("/message/content")
        .and_then(|c| c.as_array())
        .and_then(|arr| {
            arr.iter().find_map(|b| {
                if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                    b.get("text")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                }
            })
        })
        .or_else(|| {
            evt.pointer("/message/content")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string())
        });
    let Some(text) = text else { return };
    if text.trim().is_empty() {
        return;
    }
    let model = app.get_messages();
    let Some(vm) = model.as_any().downcast_ref::<VecModel<MsgBlock>>() else {
        return;
    };
    vm.push(MsgBlock {
        kind: "text".into(),
        role: "user".into(),
        text: text.into(),
        toolName: "".into(),
        toolStatus: "".into(),
        expanded: false,
        toolId: "".into(),
        codeLang: "".into(),
        time: now_time().into(),
        messageId: "".into(),
        blockIndex: 0,
    });
}

/// Push a system error block into the live model (stderr / bridge_error),
/// skipping consecutive duplicates.
fn push_live_system_error(app: &App, text: &str) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    let model = app.get_messages();
    let Some(vm) = model.as_any().downcast_ref::<VecModel<MsgBlock>>() else {
        return;
    };
    let dup = vm
        .row_count()
        .checked_sub(1)
        .and_then(|r| vm.row_data(r))
        .map(|b| b.kind == "error" && b.text.trim() == trimmed)
        .unwrap_or(false);
    if dup {
        return;
    }
    vm.push(MsgBlock {
        kind: "error".into(),
        role: "system".into(),
        text: trimmed.into(),
        toolName: "".into(),
        toolStatus: "".into(),
        expanded: false,
        toolId: "".into(),
        codeLang: "".into(),
        time: now_time().into(),
        messageId: "".into(),
        blockIndex: 0,
    });
}

// iOS entry point, called from the Obj-C UIApplicationDelegate
// (mobile/ios/Sources/AppDelegate.mm) once UIKit is ready. The main thread
// runs only the Slint event loop; all network I/O happens on the background
// thread spawned by `spawn_net_thread` (which owns its own tokio runtime).
#[cfg(target_os = "ios")]
#[no_mangle]
pub extern "C" fn synapse_ios_main() {
    // DEBUG: auto-connect for iOS simulator testing. Hardcode env vars
    // because SIMCTL_CHILD_ env injection does not reach a Slint iOS app.
    std::env::set_var("SYNAPSE_HOST", "127.0.0.1");
    std::env::set_var("SYNAPSE_PORT", "4173");
    std::env::set_var("SYNAPSE_TOKEN", "CODE");
    let _ = run_app();
}

/// Start the embedded web chat host and return the URL the iOS WKWebView should
/// load, as a malloc'd C string the caller must `free`. The URL carries the
/// pairing credentials as query params so the web app connects without a
/// JS<->native bridge. Reads SYNAPSE_HOST/PORT/TOKEN/TLS from the environment
/// (the iOS entry sets these; a real pairing flow would set them post-scan).
///
/// Returns null on host-start failure. The host runs for the process lifetime.
///
/// NOTE: Not yet exercised on-device — the iOS WKWebView host (mobile/ios) that
/// calls this needs an Xcode build to verify end-to-end.
#[cfg(target_os = "ios")]
#[no_mangle]
pub extern "C" fn synapse_web_url() -> *mut std::os::raw::c_char {
    let port = match web::spawn_host() {
        Ok(p) => p,
        Err(_) => return std::ptr::null_mut(),
    };
    let host = std::env::var("SYNAPSE_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let sport = std::env::var("SYNAPSE_PORT").unwrap_or_else(|_| "4173".into());
    let token = std::env::var("SYNAPSE_TOKEN").unwrap_or_default();
    let tls = std::env::var("SYNAPSE_TLS")
        .map(|v| v == "1")
        .unwrap_or(false);
    let url = format!(
        "http://127.0.0.1:{port}/?host={host}&port={sport}&token={token}&tls={}",
        if tls { "1" } else { "0" }
    );
    std::ffi::CString::new(url)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

// UI-thread-local weak handle to the App, used by the iOS keyboard-frame
// observer to drive `keyboardInset`. UIKit delivers keyboard notifications on
// the main (== UI) thread, so this never crosses threads.
#[cfg(target_os = "ios")]
thread_local! {
    static IOS_APP: std::cell::RefCell<slint::Weak<App>> =
        std::cell::RefCell::new(slint::Weak::default());
}

/// Install the App weak so `synapse_set_keyboard_inset` can reach it. Called
/// once from `run_app` on iOS (the UI thread).
#[cfg(target_os = "ios")]
fn ios_install_app(app: &App) {
    IOS_APP.with(|c| *c.borrow_mut() = app.as_weak());
}

/// Called from the UIKit keyboard-frame observer (mobile/ios/Sources/main.m).
/// `pts` is the on-screen keyboard height in points (== Slint logical length on
/// iOS). Drives the composer's bottom inset so it stays visible above the
/// keyboard. No-op when the app handle is gone.
#[cfg(target_os = "ios")]
#[no_mangle]
pub extern "C" fn synapse_set_keyboard_inset(pts: f32) {
    let weak = IOS_APP.with(|c| c.borrow().clone());
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            app.set_keyboardInset(pts.max(0.0));
        }
    });
}

// ===================== streaming protocol parser =====================
//
// The server forwards every Claude Code stream-json event verbatim, wrapped
// as `{"type":"event","event":<raw>}`. `handle_event`'s "event" arm unwraps
// the inner `evt`; for a streamed assistant turn `evt` is one of:
//   {"type":"stream_event","event":{ "type":"message_start"|"content_block_start"|
//        "content_block_delta"|"content_block_stop"|"message_stop", ... }}
// plus the final whole-message frames {"type":"assistant",...} and the
// turn-lifecycle {"type":"system","subtype":"turn_started"|"turn_stopped"|...}.
//
// `StreamState` is the per-turn scratch: it consumes the inner `stream_event`
// sequence and accumulates blocks keyed by Anthropic `index`, tagged with the
// current `message_id`. It is PURE (no App/Slint) so it unit-tests without the
// UI. The flush layer (in `handle_event`) reconciles `state.blocks()` into the
// live VecModel by `message_id`+`blockIndex`, and the final `assistant` frame
// replaces the streamed message with the authoritative version (de-dup).

/// Streaming scratch for the active turn. Reset on `turn_stopped`/`result`.
pub struct StreamState {
    /// Anthropic message id for the message currently being streamed.
    pub message_id: String,
    /// Anthropic block `index` -> accumulated block content, in insertion order.
    blocks: Vec<(usize, MsgBlock)>,
    /// buffer for tool_use `input_json_delta` fragments, by block index.
    tool_input_buf: std::collections::HashMap<usize, String>,
    /// Set whenever a stream_event mutates state; the render timer clears it on
    /// flush so we coalesce ~480 deltas/turn into ~30 flushes/sec.
    pub dirty: bool,
}

impl Default for StreamState {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamState {
    pub fn new() -> Self {
        Self {
            message_id: String::new(),
            blocks: Vec::new(),
            tool_input_buf: Default::default(),
            dirty: false,
        }
    }

    pub fn reset(&mut self) {
        self.message_id.clear();
        self.blocks.clear();
        self.tool_input_buf.clear();
        self.dirty = false;
    }

    /// The blocks for the currently-streaming message, in order. Each block's
    /// `messageId`/`blockIndex` are set so the flush layer can upsert by key.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    pub fn block_at(&self, i: usize) -> Option<&MsgBlock> {
        self.blocks.get(i).map(|(_, b)| b)
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    fn get_block_mut(&mut self, idx: usize) -> Option<&mut MsgBlock> {
        self.blocks
            .iter_mut()
            .find(|(i, _)| *i == idx)
            .map(|(_, b)| b)
    }

    /// Consume one forwarded event (the INNER evt: the `stream_event` wrapper or
    /// a top-level `assistant`/`user`/`result`/`system` frame).
    pub fn apply(&mut self, evt: &serde_json::Value) {
        let ty = evt.get("type").and_then(|v| v.as_str()).unwrap_or("");
        // Only `stream_event` deltas mutate StreamState. A final `assistant` frame
        // is authoritative-complete: keep the streamed blocks (the flush layer
        // reconciles/replaces by id); other frames need no StreamState mutation.
        if ty == "stream_event" {
            self.apply_anthropic_event(evt.get("event").unwrap_or(&serde_json::Value::Null));
            self.dirty = true;
        }
    }

    fn apply_anthropic_event(&mut self, ev: &serde_json::Value) {
        let et = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match et {
            "message_start" => {
                let id = ev
                    .pointer("/message/id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !id.is_empty() {
                    self.message_id = id;
                }
                // Fresh block set for this message.
                self.blocks.clear();
                self.tool_input_buf.clear();
            }
            "content_block_start" => {
                let idx = ev.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let cb = ev.get("content_block").unwrap_or(&serde_json::Value::Null);
                let bt = cb.get("type").and_then(|v| v.as_str()).unwrap_or("text");
                // Don't duplicate if a delta already opened it.
                if self.get_block_mut(idx).is_some() {
                    return;
                }
                // Render kind: Anthropic "tool_use" -> MsgBlock kind "tool".
                let kind = match bt {
                    "tool_use" => "tool",
                    other => other,
                };
                let mut block = default_block(kind, &self.message_id, idx);
                match bt {
                    "tool_use" => {
                        block.toolName = cb
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("tool")
                            .into();
                        block.toolId = cb.get("id").and_then(|v| v.as_str()).unwrap_or("").into();
                        block.toolStatus = "running".into();
                    }
                    "thinking" => {
                        block.text = cb
                            .get("thinking")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .into();
                    }
                    "text" => {
                        block.text = cb.get("text").and_then(|v| v.as_str()).unwrap_or("").into();
                    }
                    _ => {}
                }
                self.blocks.push((idx, block));
            }
            "content_block_delta" => {
                let idx = ev.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let delta = ev.get("delta").unwrap_or(&serde_json::Value::Null);
                let dt = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                // Open the block lazily if a start was missed.
                if self.get_block_mut(idx).is_none() {
                    self.blocks
                        .push((idx, default_block("text", &self.message_id, idx)));
                }
                let pos = match self.blocks.iter().position(|(i, _)| *i == idx) {
                    Some(p) => p,
                    None => return,
                };
                match dt {
                    "text_delta" => {
                        let t = delta.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        let block = &mut self.blocks[pos].1;
                        block.text = format!("{}{}", block.text, t).into();
                    }
                    "thinking_delta" => {
                        let t = delta.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                        let block = &mut self.blocks[pos].1;
                        block.text = format!("{}{}", block.text, t).into();
                    }
                    "input_json_delta" => {
                        let pj = delta
                            .get("partial_json")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let buf = self.tool_input_buf.entry(idx).or_default();
                        buf.push_str(pj);
                        let parsed = serde_json::from_str::<serde_json::Value>(buf).ok();
                        // Re-borrow the block AFTER the `buf` borrow ends.
                        if let Some(input) = parsed {
                            let block = &mut self.blocks[pos].1;
                            let tool_name = block.toolName.to_string();
                            block.text = tool_arg_preview(&tool_name, Some(&input)).into();
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                // Finalize: tool_input_delta already re-parsed per-fragment.
            }
            "message_stop" => {
                // Message complete; blocks retained until reset on turn end.
            }
            _ => {}
        }
    }
}

fn default_block(kind: &str, message_id: &str, idx: usize) -> MsgBlock {
    MsgBlock {
        kind: kind.into(),
        role: "assistant".into(),
        text: "".into(),
        toolName: "".into(),
        toolStatus: "".into(),
        expanded: false,
        toolId: "".into(),
        codeLang: "".into(),
        time: "".into(),
        messageId: message_id.into(),
        blockIndex: idx as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_pure_prose() {
        let segs = split_markdown("hello world");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].0, "text");
        assert_eq!(segs[0].1, "hello world");
        assert!(segs[0].2.is_empty());
    }

    #[test]
    fn split_one_code_block() {
        let md = "before\n```rust\nfn main() {}\n```\nafter";
        let segs = split_markdown(md);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].0, "text");
        assert_eq!(segs[0].1, "before");
        assert_eq!(segs[1].0, "code");
        assert_eq!(segs[1].1, "fn main() {}");
        assert_eq!(segs[1].2, "rust");
        assert_eq!(segs[2].0, "text");
        assert_eq!(segs[2].1, "after");
    }

    #[test]
    fn split_unterminated_fence_keeps_raw() {
        // While streaming, an unclosed fence must not be dropped.
        let md = "intro\n```py\nprint(1)";
        let segs = split_markdown(md);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].0, "text");
        assert_eq!(segs[1].0, "text"); // unterminated -> stays as text
        assert!(segs[1].1.contains("print(1)"));
    }

    #[test]
    fn normalize_extracts_code_from_assistant_text() {
        let mut msgs = vec![MsgBlock {
            kind: "text".into(),
            role: "assistant".into(),
            text: "here is code:\n```sh\necho hi\n```\ndone".into(),
            toolName: "".into(),
            toolStatus: "".into(),
            expanded: false,
            toolId: "".into(),
            codeLang: "".into(),
            time: "".into(),
            messageId: "".into(),
            blockIndex: 0,
        }];
        normalize_code_blocks(&mut msgs);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].kind, "text");
        assert_eq!(msgs[1].kind, "code");
        assert_eq!(msgs[1].codeLang, "sh");
        assert_eq!(msgs[1].text, "echo hi");
        assert_eq!(msgs[2].kind, "text");
        assert_eq!(msgs[2].text, "done");
    }

    #[test]
    fn normalize_leaves_tool_blocks_untouched() {
        let mut msgs = vec![
            MsgBlock {
                kind: "tool".into(),
                role: "assistant".into(),
                text: "ls".into(),
                toolName: "Bash".into(),
                toolStatus: "running".into(),
                expanded: false,
                toolId: "t1".into(),
                codeLang: "".into(),
                time: "".into(),
                messageId: "".into(),
                blockIndex: 0,
            },
            MsgBlock {
                kind: "text".into(),
                role: "user".into(),
                text: "```not code```".into(),
                toolName: "".into(),
                toolStatus: "".into(),
                expanded: false,
                toolId: "".into(),
                codeLang: "".into(),
                time: "".into(),
                messageId: "".into(),
                blockIndex: 0,
            },
        ];
        normalize_code_blocks(&mut msgs);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].kind, "tool");
        assert_eq!(msgs[1].kind, "text"); // user text never split
    }

    #[test]
    fn short_basename_basic() {
        assert_eq!(short_basename("/Users/zx/code/synapse"), "synapse");
        assert_eq!(short_basename("~/code/foo/"), "foo");
        assert_eq!(short_basename("bar"), "bar");
        assert_eq!(short_basename(""), "");
    }

    // ---- streaming protocol parser ----

    fn se(json: &str) -> serde_json::Value {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn stream_message_start_records_id_and_opens_no_blocks() {
        let mut s = StreamState::new();
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[]}}}"#));
        assert_eq!(s.message_id, "msg_1");
        assert_eq!(s.block_count(), 0);
    }

    #[test]
    fn stream_text_block_start_then_deltas_accumulate_in_order() {
        let mut s = StreamState::new();
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[]}}}"#));
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}"#));
        assert_eq!(s.block_count(), 1);
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}}"#));
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world"}}}"#));
        let b = s.block_at(0).unwrap();
        assert_eq!(b.kind, "text");
        assert_eq!(b.text, "Hello world");
        assert_eq!(b.messageId, "msg_1");
        assert_eq!(b.blockIndex, 0);
    }

    #[test]
    fn stream_tool_use_block_records_name_id_and_input_preview() {
        let mut s = StreamState::new();
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[]}}}"#));
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_1","name":"Bash","input":{}}}}"#));
        let b = s.block_at(0).unwrap();
        assert_eq!(b.kind, "tool");
        assert_eq!(b.toolName, "Bash");
        assert_eq!(b.toolId, "call_1");
        assert_eq!(b.toolStatus, "running");
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"ls -la\"}"}}}"#));
        let b = s.block_at(0).unwrap();
        assert!(
            b.text.contains("ls -la"),
            "preview should contain the command, got: {}",
            b.text
        );
    }

    #[test]
    fn stream_thinking_block_accumulates_reasoning() {
        let mut s = StreamState::new();
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[]}}}"#));
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}}"#));
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"reasoning "}}}"#));
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"here"}}}"#));
        let b = s.block_at(0).unwrap();
        assert_eq!(b.kind, "thinking");
        assert_eq!(b.text, "reasoning here");
    }

    #[test]
    fn stream_multiple_blocks_keep_distinct_indices_in_order() {
        let mut s = StreamState::new();
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[]}}}"#));
        // thinking at index 0, text at index 1
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}}"#));
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"h"}}}"#));
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}}"#));
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"answer"}}}"#));
        assert_eq!(s.block_count(), 2);
        assert_eq!(s.block_at(0).unwrap().kind, "thinking");
        assert_eq!(s.block_at(1).unwrap().kind, "text");
        assert_eq!(s.block_at(1).unwrap().text, "answer");
    }

    #[test]
    fn stream_unknown_event_is_noop() {
        let mut s = StreamState::new();
        s.apply(&se(
            r#"{"type":"stream_event","event":{"type":"ping","index":0}}"#,
        ));
        assert_eq!(s.block_count(), 0);
        // Non-stream_event frames are ignored by the parser (handled elsewhere).
        s.apply(&se(
            r#"{"type":"assistant","message":{"id":"x","content":[]}}"#,
        ));
        assert_eq!(s.block_count(), 0);
    }

    #[test]
    fn stream_reset_clears_state() {
        let mut s = StreamState::new();
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[]}}}"#));
        s.apply(&se(r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"hi"}}}"#));
        assert!(!s.is_empty());
        s.reset();
        assert!(s.message_id.is_empty());
        assert!(s.is_empty());
    }
}

#[cfg(test)]
mod pair_tests {
    use super::net::parse_pair_link;

    #[test]
    fn parse_full_synapse_link() {
        let p = parse_pair_link("synapse://192.168.1.6:4173?token=CODE&tls=1").unwrap();
        assert_eq!(p.host, "192.168.1.6");
        assert_eq!(p.port, "4173");
        assert_eq!(p.token, "CODE");
        assert!(p.tls);
    }

    #[test]
    fn parse_no_tls_defaults_false() {
        let p = parse_pair_link("synapse://example.com:443?token=abc").unwrap();
        assert_eq!(p.host, "example.com");
        assert_eq!(p.port, "443");
        assert_eq!(p.token, "abc");
        assert!(!p.tls);
    }

    #[test]
    fn parse_tls_zero_is_false() {
        let p = parse_pair_link("synapse://h:1?token=t&tls=0").unwrap();
        assert!(!p.tls);
    }

    #[test]
    fn parse_rejects_missing_token() {
        assert!(parse_pair_link("synapse://h:1").is_none());
        assert!(parse_pair_link("synapse://h:1?tls=1").is_none());
    }

    #[test]
    fn parse_relay_link_with_connect_path() {
        // Relay pairing URL: synapse://relay.example.com/connect?deviceId=abc&token=t&tls=1
        let p =
            parse_pair_link("synapse://relay.example.com/connect?deviceId=abc123&token=xyz&tls=1")
                .unwrap();
        assert_eq!(p.host, "relay.example.com");
        assert_eq!(p.port, "443"); // relay uses standard wss port
        assert_eq!(p.token, "xyz");
        assert!(p.tls);
        assert_eq!(p.path, "/connect");
    }

    #[test]
    fn parse_defaults_port_from_tls() {
        // No port + tls=1 => 443 (relay on standard wss port)
        let p = parse_pair_link("synapse://relay.io/connect?token=t&tls=1").unwrap();
        assert_eq!(p.port, "443");
        // No port + no tls => 80
        let p2 = parse_pair_link("synapse://host?token=t").unwrap();
        assert_eq!(p2.port, "80");
    }

    #[test]
    fn parse_tolerates_whitespace_and_no_scheme() {
        let p = parse_pair_link("  10.0.0.5:4173?token=XYZ&tls=true  ").unwrap();
        assert_eq!(p.host, "10.0.0.5");
        assert_eq!(p.token, "XYZ");
        assert!(p.tls);
    }
}
