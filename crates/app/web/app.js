// Synapse chat web app. A WS client that owns the whole post-pairing experience:
// connection lifecycle, reconnect backoff, protocol ops, and rendering. Mirrors
// the contract in crates/app/src/net.rs + handle_event/assemble_assistant_blocks.

(() => {
"use strict";

// marked + highlight config
marked.setOptions({ breaks: true, gfm: true });

const $ = (id) => document.getElementById(id);
const messagesEl = $("messages");
const scroller = $("scroller");
const emptyEl = $("empty");
const inputEl = $("input");
const sendBtn = $("sendBtn");
const titleName = $("titleName");

const state = {
  ws: null,
  url: "",
  backoff: 1000,
  connected: false,
  busy: false,
  view: "sessions", // "sessions" | "chat"
  activeId: "",
  sessions: [],
  models: [],          // model catalog from hello: [{id,label}]
  defaultModel: "",    // pre-selected model id for new sessions
  pendingModel: null,  // model chosen before a session exists; used on create
  cwds: [],            // project working dirs from hello (git repos)
  pendingCwd: null,    // project chosen for the next create
  pendingMode: null,   // permission mode chosen before a session exists
  pendingSend: null,   // message queued while a new session is being created
  blocks: [],         // rendered message elements (for empty/clear bookkeeping)
  // The current assistant turn. Synara model: a turn's thinking + tool calls are
  // "work"; while running they show live, and once the turn settles they collapse
  // into a single "Worked for Xs ›" disclosure above the final reply text.
  //   { el, workWrap, workBody, replyWrap, items:[], tools:Map, text, startMs }
  turn: null,
  msNow: 0,           // monotonic-ish clock fed from frames (no Date in workflow ctx, fine here)
};

// ---- credential injection: native / URL / localStorage ----
const CREDS_KEY = "synapse_creds";

function creds() {
  if (window.__SYNAPSE__) return window.__SYNAPSE__;
  const p = new URLSearchParams(location.search);
  const h = p.get("host"), tok = p.get("token");
  if (h && tok) {
    return {
      host: h,
      port: p.get("port") || "4173",
      token: tok,
      tls: p.get("tls") === "1",
      path: p.get("path") || "",
    };
  }
  try {
    const raw = localStorage.getItem(CREDS_KEY);
    if (raw) return JSON.parse(raw);
  } catch {}
  return null;
}

function persistCreds(c) {
  try { localStorage.setItem(CREDS_KEY, JSON.stringify(c)); } catch {}
}

function clearCreds() {
  try { localStorage.removeItem(CREDS_KEY); } catch {}
}

// Parse synapse:// / wss:// pairing links (mirrors crates/app/src/net.rs).
function parsePairLink(link) {
  const raw = link.trim();
  if (!raw) return null;
  let body = raw
    .replace(/^synapse:\/\//, "")
    .replace(/^synapse:/, "")
    .replace(/^wss:\/\//, "")
    .replace(/^ws:\/\//, "");
  const qIdx = body.indexOf("?");
  const authPath = qIdx >= 0 ? body.slice(0, qIdx) : body;
  const query = qIdx >= 0 ? body.slice(qIdx + 1) : "";
  const params = new URLSearchParams(query);
  const token = params.get("token") || "";
  const tls = params.get("tls") === "1" || params.get("tls") === "true";
  if (!token) return null;

  const slash = authPath.indexOf("/");
  if (slash >= 0) {
    const authority = authPath.slice(0, slash);
    const path = authPath.slice(slash);
    const { host, port } = splitHostPort(authority, tls);
    if (!host) return null;
    return { host, port, token, tls, path };
  }
  const { host, port } = splitHostPort(authPath, tls);
  if (!host) return null;
  return { host, port, token, tls, path: "" };
}

function splitHostPort(authority, tls) {
  const i = authority.lastIndexOf(":");
  if (i > 0) {
    const port = authority.slice(i + 1);
    if (/^\d+$/.test(port)) {
      return { host: authority.slice(0, i), port };
    }
  }
  return { host: authority, port: tls ? "443" : "80" };
}

function showConnectOverlay() {
  $("connectOverlay").classList.remove("hidden");
}
function hideConnectOverlay() {
  $("connectOverlay").classList.add("hidden");
}

function applyCreds(c) {
  window.__SYNAPSE__ = c;
  persistCreds(c);
  hideConnectOverlay();
  state.url = buildUrl(c);
  state.backoff = 1000;
  doConnect(true);
}

function pairFromForm() {
  const link = ($("pairLink").value || "").trim();
  if (link) {
    const c = parsePairLink(link);
    if (!c) { toast("Invalid pairing link"); return; }
    applyCreds(c);
    return;
  }
  const host = ($("pairHost").value || "").trim();
  const token = ($("pairToken").value || "").trim();
  if (!host || !token) { toast("Host and token required"); return; }
  applyCreds({
    host,
    port: ($("pairPort").value || "4173").trim(),
    token,
    tls: $("pairTls").checked,
    path: "",
  });
}

function buildUrl(c) {
  const scheme = c.tls ? "wss" : "ws";
  if (c.path) return `${scheme}://${c.host}:${c.port}${c.path}?token=${c.token}`;
  return `${scheme}://${c.host}:${c.port}/?token=${c.token}`;
}

// =================== connection ===================
function connect() {
  const c = creds();
  if (!c) { showConnectOverlay(); return; }
  window.__SYNAPSE__ = c;
  state.url = buildUrl(c);
  doConnect(true);
}

function doConnect(first) {
  try { state.ws = new WebSocket(state.url); }
  catch (e) { scheduleReconnect(first); return; }
  state.ws.onopen = () => {
    state.connected = true;
    state.backoff = 1000;
    if (window.__SYNAPSE__) persistCreds(window.__SYNAPSE__);
    hideConnectOverlay();
    $("reconnect").classList.remove("show");
    showSessions();
    // prime session list
    send({ op: "list" });
    if (first) {
      // pick first session after list arrives; nothing else here
    } else if (state.activeId) {
      send({ op: "history", sessionId: state.activeId, limit: 400 });
    }
  };
  state.ws.onmessage = (ev) => {
    let v; try { v = JSON.parse(ev.data); } catch { return; }
    handle(v);
  };
  state.ws.onclose = () => {
    state.connected = false;
    if (first) {
      toast("Could not connect — check link and server");
      showConnectOverlay();
    } else {
      $("reconnect").classList.add("show");
      scheduleReconnect(false);
    }
  };
  state.ws.onerror = () => { /* onclose will follow */ };
}

function scheduleReconnect(first) {
  if (first) return;
  const d = state.backoff;
  state.backoff = Math.min(d * 2, 15000);
  setTimeout(() => doConnect(false), d);
}

function send(obj) {
  if (state.ws && state.ws.readyState === WebSocket.OPEN) {
    state.ws.send(JSON.stringify(obj));
  }
}

// =================== inbound dispatch ===================
function handle(v) {
  switch (v.type) {
    case "hello":
      state.models = v.models || [];
      state.defaultModel = v.defaultModel || "";
      state.cwds = v.cwds || [];
      setSessions(v.sessions || []);
      syncModelLabel();
      syncLocalLabel();
      break;
    case "sessions": setSessions(v.sessions || []); break;
    case "created":
      setSessions(state.sessions); // list update follows via event
      select(v.session.id);
      // The pick has been consumed by create; clear it so currentCwd()/
      // currentModelId() never report a stale choice before the next session.
      state.pendingCwd = null; state.pendingModel = null;
      if (state.pendingSend) {
        const t = state.pendingSend; state.pendingSend = null;
        send({ op: "send", sessionId: v.session.id, content: t });
      }
      break;
    case "sessions": break;
    case "history":
      // Ignore a stale response for a session we've navigated away from: a slow
      // history (large transcript, mobile link) can land after the user switched,
      // painting the wrong session's messages under the new one ("session错乱").
      // Mirrors the live-event guard below.
      if (v.sessionId && v.sessionId !== state.activeId) break;
      if (v.found !== false) ingestHistory(v.events || []);
      break;
    case "event": handleEvent(v.event); break;
    case "error":
      toast(typeof v.error === "string" ? v.error : "error");
      break;
  }
}

function handleEvent(evt) {
  const t = evt.type;
  const sub = evt.subtype;
  // Multi-device: the server broadcasts every session's events to every client.
  // Session lifecycle and turn status keep EVERY session's drawer state fresh
  // (so the busy dot and busy-on-open reflect turns started on other devices);
  // all other events are transcript output for one session and are dropped
  // unless we're viewing that session.
  if (t === "system" && (sub === "session_created" || sub === "session_updated")) {
    upsertSession(evt.session);
    if (sub === "session_updated" && evt.sessionId === state.activeId) { syncModelLabel(); syncLocalLabel(); syncPermLabel(); }
    return;
  }
  if (t === "system" && sub === "session_deleted") {
    const i = state.sessions.findIndex(x => x.id === evt.sessionId);
    if (i >= 0) state.sessions.splice(i, 1);
    if (evt.sessionId === state.activeId) {
      state.activeId = ""; clearMessages(); titleName.textContent = workspaceName(); setBusy(false);
      showSessions();
    }
    renderSessions();
    return;
  }
  if (t === "system" && (sub === "turn_started" || sub === "turn_stopped" || sub === "bridge_error")) {
    setSessionState(evt.sessionId, sub === "turn_started" ? "busy" : (sub === "bridge_error" ? "error" : "idle"));
    if (evt.sessionId === state.activeId) {
      if (sub === "turn_started") { startTurn(); setBusy(true); }
      else if (sub === "turn_stopped") { setBusy(false); finalizeStream(); }
      else { pushError(str(evt.error) || "Turn failed"); setBusy(false); finalizeStream(); }
    }
    return;
  }
  if (evt.sessionId && evt.sessionId !== state.activeId) return;
  if (typeof evt.ttft_ms === "number") state.msNow += 0; // (kept simple; elapsed uses counters below)
  if (t === "system") { /* api_retry / fallback_to_json: no-op for the open view */ return; }
  if (t === "assistant") { ingestAssistant(evt); return; }
  if (t === "permission_request") { showPermission(evt); return; }
  if (t === "user") {
    if (evt.message) {
      // A user frame is either the echoed human prompt (has text) or a tool
      // result the model's tool produced (has tool_result blocks). Route the
      // latter into the active turn so tool cards fill in their output live,
      // not only after a history reload.
      const content = evt.message.content;
      if (Array.isArray(content) && content.some(c => c && c.type === "tool_result")) {
        ingestResult(evt);
      }
      const txt = contentText(content);
      if (txt) echoUser(txt, evt.message.id);
    }
    return;
  }
  if (t === "result") { ingestResult(evt); return; }
  if (t === "stderr") {
    const txt = str(evt.text);
    if (txt) pushStderr(txt);
    return;
  }
}

// =================== turn model ===================
// A turn owns two regions inside one assistant message element:
//   .work  — thinking + tool calls (live while running, collapsed when settled)
//   .reply — the final markdown answer
function startTurn() {
  // close any previous turn first (defensive; turn_stopped normally does this)
  if (state.turn) finalizeStream();
  const el = mkMsg("assistant");
  const body = el.querySelector(".body");
  const workWrap = document.createElement("div"); workWrap.className = "work";
  const replyWrap = document.createElement("div"); replyWrap.className = "reply";
  body.appendChild(workWrap); body.appendChild(replyWrap);
  state.turn = {
    el, workWrap, replyWrap,
    tools: new Map(),     // tool_use_id -> {id,name,input,status,output}
    // Everything in arrival order. Synara model: the TRAILING text segment is the
    // reply; thinking, tools, and any earlier text fold into "Worked for Xs".
    order: [],            // [{kind:'thinking'|'text', text} | {kind:'tool', id}]
    ticks: 0,             // live elapsed: counts seconds via pulse timer
    firstTs: 0, lastTs: 0,// frame timestamps (ms) — elapsed source for history
    appended: false,
  };
  // don't add to DOM until there's content (avoid empty box)
}

function ensureTurnInDom() {
  const tn = state.turn;
  if (tn && !tn.appended) { addBlock(tn.el); tn.appended = true; }
}

function ingestAssistant(evt) {
  const msg = evt.message;
  if (!msg) return;
  const content = Array.isArray(msg.content)
    ? msg.content
    : (typeof msg.content === "string" ? [{ type: "text", text: msg.content }] : []);

  // Genuine auth/quota/API error frame: explicit error marker.
  const isError = evt.error || msg.error || evt.isApiErrorMessage;
  if (isError) {
    const et = (content[0] && content[0].text) || str(evt.error) || str(msg.error) || "Request failed";
    pushError(str(et));
    return;
  }

  // No active turn (e.g. history backfill) → start one implicitly.
  if (!state.turn) startTurn();
  const tn = state.turn;
  // Track frame timestamps so history (which has no live timer) can still show
  // a real "Worked for Xs".
  const ts = evt.timestamp ? Date.parse(evt.timestamp) : 0;
  if (ts) { if (!tn.firstTs) tn.firstTs = ts; tn.lastTs = ts; }

  for (const blk of content) {
    if (blk.type === "text" && typeof blk.text === "string") {
      appendSeg(tn, "text", blk.text);
    } else if (blk.type === "thinking" && typeof blk.thinking === "string" && blk.thinking) {
      appendSeg(tn, "thinking", blk.thinking);
    } else if (blk.type === "tool_use") {
      const tool = {
        id: blk.id, name: blk.name,
        args: (blk.input && typeof blk.input === "object") ? blk.input : {},
        input: typeof blk.input === "string" ? blk.input : JSON.stringify(blk.input ?? {}, null, 2),
        status: "running", output: "",
      };
      tn.tools.set(blk.id, tool);
      tn.order.push({ kind: "tool", id: blk.id });
    }
  }

  const hasContent = tn.order.length > 0;
  if (hasContent) { ensureTurnInDom(); renderTurn(false); }
  updatePulse();
  ensurePinned();
}

// Merge consecutive same-kind segments (streamed text/thinking arrive as deltas);
// each tool_use gets its own ordered slot.
function appendSeg(tn, kind, text) {
  const last = tn.order[tn.order.length - 1];
  if (last && last.kind === kind) last.text += text;
  else tn.order.push({ kind, text });
}

// Synara split: the trailing text segment is the visible reply; everything before
// it (thinking, tools, earlier text) is "work" that collapses into "Worked for Xs".
function splitTurn(tn) {
  const last = tn.order[tn.order.length - 1];
  if (last && last.kind === "text") return { replyText: last.text, workItems: tn.order.slice(0, -1) };
  return { replyText: "", workItems: tn.order };
}

// Render the active turn. While running, work shows in a fixed-height activity
// feed that scrolls upward as new steps arrive; when settled it collapses to one line.
function renderTurn(settled) {
  const tn = state.turn;
  if (!tn) return;
  const { replyText, workItems } = splitTurn(tn);
  const hasWork = workItems.length > 0 || (state.busy && !settled);

  tn.workWrap.innerHTML = "";
  if (hasWork) {
    if (settled) {
      const tsSecs = tn.lastTs > tn.firstTs ? Math.round((tn.lastTs - tn.firstTs) / 1000) : 0;
      const secs = tn.ticks > 0 ? tn.ticks : tsSecs;
      const summary = summarizeWork(workItems, tn, secs);
      const line = statusLine(summary, () => openWorkSheet(workItems, tn, secs), "work-summary has-action");
      tn.workWrap.appendChild(line);
    } else {
      const feed = document.createElement("div");
      feed.className = "activity-feed";
      const scroll = document.createElement("div");
      scroll.className = "activity-scroll";
      feed.appendChild(scroll);
      tn.workWrap.appendChild(feed);
      tn.activityFeed = feed;
      renderActivityFeed(scroll, workItems, tn);
      scrollActivityFeed(feed, scroll);
    }
  }

  tn.replyWrap.innerHTML = "";
  if (replyText) tn.replyWrap.appendChild(mdEl(replyText));
}

function scrollActivityFeed(feed, scroll) {
  requestAnimationFrame(() => {
    const overflow = scroll.scrollHeight - feed.clientHeight;
    if (overflow > 0) scroll.style.transform = `translateY(-${overflow}px)`;
    else scroll.style.transform = "";
  });
}

function renderActivityFeed(scroll, items, tn) {
  scroll.innerHTML = "";
  const lastIdx = items.length - 1;
  for (let i = 0; i < items.length; i++) {
    const it = items[i];
    if (i === lastIdx && state.busy && isItemActive(it, tn)) continue;
    appendActivityLine(scroll, it, tn, i, items);
  }
  const now = activeItemLine(items, tn);
  if (now) {
    const cls = "status-now has-action" + (now.running ? " running" : "");
    scroll.appendChild(statusLine(now.label, now.onClick, cls));
  }
}

function isItemActive(it, tn) {
  if (it.kind === "thinking") return isThinkingActive(tn, it);
  if (it.kind === "tool") {
    const t = tn.tools.get(it.id);
    return t && t.status === "running";
  }
  return false;
}

function appendActivityLine(scroll, it, tn, idx, items) {
  if (it.kind === "thinking") {
    const secs = thinkingSecsForItem(tn, it, idx, items);
    scroll.appendChild(statusLine(
      secs > 0 ? `Thought ${fmtElapsed(secs)}` : "Thought",
      () => openThinkingSheet(it.text, secs),
      "has-action"
    ));
  } else if (it.kind === "tool") {
    const t = tn.tools.get(it.id);
    if (!t) return;
    scroll.appendChild(statusLine(toolStatusLabel(t), () => openToolSheet(t), "has-action"));
  }
}

function thinkingSecsForItem(tn, item, idx, items) {
  if (idx < items.length - 1) {
    const next = items[idx + 1];
    if (next && next.kind === "tool") return Math.max(1, thinkingSecs(tn));
  }
  const secs = thinkingSecs(tn);
  return secs > 0 ? secs : 1;
}

function activeItemLine(items, tn) {
  if (!state.busy) return null;
  const last = items[items.length - 1];
  if (last && last.kind === "text" && last.text) return null;
  if (!last) return { label: "Planning next moves", running: true };
  if (last.kind === "thinking" && isThinkingActive(tn, last)) {
    const secs = thinkingSecs(tn);
    return {
      label: secs > 0 ? `Thought ${fmtElapsed(secs)}` : "Thinking…",
      running: true,
      onClick: () => openThinkingSheet(last.text, secs),
    };
  }
  if (last.kind === "tool") {
    const t = tn.tools.get(last.id);
    if (t && t.status === "running") {
      return { label: currentToolAction(t), running: true, onClick: () => openToolSheet(t) };
    }
  }
  return { label: "Planning next moves", running: true };
}

function currentToolAction(t) {
  const a = t.args || {};
  const file = basename(a.file_path || a.path || a.notebook_path || "");
  switch (t.name) {
    case "Edit": case "Write": case "MultiEdit": case "NotebookEdit":
      return file ? `Editing ${file}` : "Editing";
    case "Read": case "LS":
      return file ? `Reading ${file}` : "Reading";
    case "Grep": return `Searching ${str(a.pattern)}`;
    case "Glob": return `Searching ${str(a.pattern)}`;
    case "Bash": return "Running command";
    case "Task": return "Running task";
    case "WebSearch": return `Searching ${firstLine(str(a.query))}`;
    case "WebFetch": return `Fetching ${hostOf(a.url)}`;
    case "TodoWrite": return "Updating plan";
    default: return toolMeta(t).title;
  }
}

function statusLine(label, onClick, extraClass) {
  const btn = document.createElement("button");
  btn.className = "status-line" + (extraClass ? " " + extraClass : "");
  if (onClick) {
    btn.classList.add("has-action");
    btn.innerHTML = `${escapeHtml(label)}<span class="chev"> ›</span>`;
    btn.addEventListener("click", onClick);
  } else {
    btn.textContent = label;
  }
  return btn;
}

function thinkingSecs(tn) {
  const tsSecs = tn.lastTs > tn.firstTs ? Math.round((tn.lastTs - tn.firstTs) / 1000) : 0;
  return tn.ticks > 0 ? tn.ticks : tsSecs;
}

function isThinkingActive(tn, thinkingItem) {
  const idx = tn.order.indexOf(thinkingItem);
  const after = tn.order.slice(idx + 1);
  return after.length === 0 || after.every(x => x.kind === "thinking");
}

function thinkingLabel(secs, running) {
  if (running) return "Thinking…";
  return secs > 0 ? `Thought ${fmtElapsed(secs)}` : "Thought";
}

function toolStatusLabel(t) {
  const a = t.args || {};
  const meta = toolMeta(t);
  const file = basename(a.file_path || a.path || a.notebook_path || "");
  if (t.status === "running") return currentToolAction(t);
  switch (t.name) {
    case "Edit": {
      const d = lineDiff(str(a.old_string), str(a.new_string));
      return `Edited ${file} ${diffStat(d)}`;
    }
    case "MultiEdit": {
      const edits = Array.isArray(a.edits) ? a.edits : [];
      let ad = 0, de = 0;
      for (const e of edits) { const d = lineDiff(str(e.old_string), str(e.new_string)); ad += d.adds; de += d.dels; }
      return `Edited ${file} +${ad} −${de}`;
    }
    case "Write": {
      const n = str(a.content).split("\n").length;
      return `Edited ${file} +${n}`;
    }
    case "NotebookEdit":
      return `Edited ${file}`;
    case "Read": case "LS":
      return file ? `Read ${file}` : "Explored files";
    case "Grep":
      return `Grepped ${str(a.pattern)}${a.path ? " in " + basename(a.path) : ""}`;
    case "Glob":
      return `Searched ${str(a.pattern)}`;
    case "Bash":
      return `Ran ${firstLine(str(a.command))}`;
    case "Task":
      return `Completed task ${meta.sub || meta.title}`;
    case "WebSearch":
      return `Searched ${firstLine(str(a.query))}`;
    case "WebFetch":
      return `Fetched ${hostOf(a.url)}`;
    case "TodoWrite":
      return "Updated plan";
    case "AskUserQuestion":
      return `Asked ${firstLine(askText(a))}`;
    case "ExitPlanMode": case "exit_plan_mode":
      return "Proposed plan";
    default:
      return `Used ${meta.title.toLowerCase()}${meta.sub ? " · " + meta.sub : ""}`;
  }
}

function summarizeWork(items, tn, secs) {
  let files = 0, searches = 0, edits = 0, other = 0, hasThinking = false;
  for (const it of items) {
    if (it.kind === "thinking") { hasThinking = true; continue; }
    if (it.kind !== "tool") continue;
    const t = tn.tools.get(it.id);
    if (!t) continue;
    if (["Read", "LS"].includes(t.name)) files++;
    else if (["Grep", "Glob", "WebSearch"].includes(t.name)) searches++;
    else if (["Edit", "Write", "MultiEdit", "NotebookEdit"].includes(t.name)) edits++;
    else other++;
  }
  const parts = [];
  if (files) parts.push(`${files} file${files > 1 ? "s" : ""}`);
  if (searches) parts.push(`${searches} search${searches > 1 ? "es" : ""}`);
  if (edits) parts.push(`${edits} edit${edits > 1 ? "s" : ""}`);
  if (other) parts.push(`${other} other tool${other > 1 ? "s" : ""}`);
  if (parts.length) return `Explored ${parts.join(", ")}`;
  if (hasThinking) return secs > 0 ? `Thought ${fmtElapsed(secs)}` : "Thought";
  return secs > 0 ? `Worked for ${fmtElapsed(secs)}` : "Details";
}

// Bottom sheet for expanded thinking / tool / work details
function openSheet(title, content) {
  $("sheetTitle").textContent = title;
  const body = $("sheetBody");
  body.innerHTML = "";
  if (typeof content === "string") {
    const pre = document.createElement("div");
    pre.className = "sheet-thinking";
    pre.textContent = content;
    body.appendChild(pre);
  } else {
    body.appendChild(content);
  }
  $("sheetMask").classList.add("show");
  $("bottomSheet").classList.add("show");
}
function closeSheet() {
  $("sheetMask").classList.remove("show");
  $("bottomSheet").classList.remove("show");
}
function openThinkingSheet(text, secs) {
  openSheet(secs > 0 ? `Thought ${fmtElapsed(secs)}` : "Thought", text || "No details.");
}
function openToolSheet(t) {
  const meta = toolMeta(t);
  const wrap = document.createElement("div");
  const card = toolCard(t);
  card.classList.add("open");
  wrap.appendChild(card);
  openSheet(meta.title + (meta.sub ? ` · ${meta.sub}` : ""), wrap);
}
function openWorkSheet(items, tn, secs) {
  const wrap = document.createElement("div");
  for (const it of items) {
    const block = document.createElement("div");
    block.className = "sheet-item";
    const lbl = document.createElement("div");
    lbl.className = "sheet-item-label";
    if (it.kind === "thinking") {
      lbl.textContent = thinkingLabel(secs, false);
      const pre = document.createElement("div");
      pre.className = "sheet-thinking";
      pre.textContent = it.text;
      block.appendChild(lbl); block.appendChild(pre);
    } else if (it.kind === "tool") {
      const t = tn.tools.get(it.id);
      if (!t) continue;
      lbl.textContent = toolStatusLabel(t);
      block.appendChild(lbl);
      block.appendChild(toolCard(t));
    } else if (it.kind === "text") {
      lbl.textContent = "Draft";
      block.appendChild(lbl);
      block.appendChild(mdEl(it.text));
    }
    wrap.appendChild(block);
  }
  const title = secs > 0 ? `Worked for ${fmtElapsed(secs)}` : "Details";
  openSheet(title, wrap);
}

// =================== tool views (per-tool rich rendering) ===================
// A tool call renders as a collapsible card (Synara file-row style). The head
// carries an at-a-glance subtitle (path +adds −dels for edits, the command for
// Bash, the pattern for searches); expanding reveals the diff / output.
const TOOL_GLYPH = {
  Edit: "✎", MultiEdit: "✎", Write: "✎", NotebookEdit: "✎",
  Read: "▤", Bash: "⌘", Grep: "⌕", Glob: "⌕", LS: "▤",
  TodoWrite: "☑", Task: "✦", WebFetch: "⚓", WebSearch: "⌕",
  ExitPlanMode: "✦", exit_plan_mode: "✦", AskUserQuestion: "✦",
};

function toolCard(t) {
  const meta = toolMeta(t);
  const card = document.createElement("div");
  card.className = "card";
  const head = document.createElement("div");
  head.className = "card-head";
  head.innerHTML =
    `<span class="ic">${meta.glyph}</span>` +
    `<span class="nm">${escapeHtml(meta.title)}${meta.sub ? `<span class="sub">${escapeHtml(meta.sub)}</span>` : ""}</span>` +
    `<span class="st ${t.status}"></span>` +
    `<span class="chev">▸</span>`;
  const body = document.createElement("div");
  body.className = "card-body";
  body.appendChild(toolBody(t));
  card.appendChild(head); card.appendChild(body);
  head.addEventListener("click", () => card.classList.toggle("open"));
  return card;
}

function toolMeta(t) {
  const a = t.args || {};
  const glyph = TOOL_GLYPH[t.name] || "✦";
  const base = (p) => basename(p || "");
  switch (t.name) {
    case "Edit": {
      const d = lineDiff(str(a.old_string), str(a.new_string));
      return { glyph, title: "Edit", sub: `${base(a.file_path)} ${diffStat(d)}` };
    }
    case "MultiEdit": {
      const edits = Array.isArray(a.edits) ? a.edits : [];
      let ad = 0, de = 0;
      for (const e of edits) { const d = lineDiff(str(e.old_string), str(e.new_string)); ad += d.adds; de += d.dels; }
      return { glyph, title: "Edit", sub: `${base(a.file_path)} ·${edits.length} +${ad} −${de}` };
    }
    case "Write":
      return { glyph, title: "Write", sub: `${base(a.file_path)} +${str(a.content).split("\n").length}` };
    case "NotebookEdit":
      return { glyph, title: "Notebook", sub: base(a.notebook_path) };
    case "Read":
      return { glyph, title: "Read", sub: base(a.file_path) };
    case "Bash":
      return { glyph, title: "Bash", sub: firstLine(str(a.command)) };
    case "Grep":
      return { glyph, title: "Grep", sub: str(a.pattern) + (a.path ? ` · ${base(a.path)}` : "") };
    case "Glob":
      return { glyph, title: "Glob", sub: str(a.pattern) };
    case "LS":
      return { glyph, title: "LS", sub: base(a.path) };
    case "TodoWrite": {
      const todos = Array.isArray(a.todos) ? a.todos : [];
      const done = todos.filter(x => x.status === "completed").length;
      return { glyph, title: "Plan", sub: `${done}/${todos.length}` };
    }
    case "Task":
      return { glyph, title: a.subagent_type ? `Task · ${a.subagent_type}` : "Task", sub: firstLine(str(a.description)) };
    case "WebFetch":
      return { glyph, title: "Fetch", sub: hostOf(a.url) };
    case "WebSearch":
      return { glyph, title: "Search", sub: firstLine(str(a.query)) };
    case "ExitPlanMode":
    case "exit_plan_mode":
      return { glyph, title: "Plan proposed", sub: "" };
    case "AskUserQuestion":
      return { glyph, title: "Question", sub: firstLine(askText(a)) };
    default:
      return { glyph, title: t.name, sub: "" };
  }
}

function toolBody(t) {
  const a = t.args || {};
  const out = t.output ? String(t.output) : "";
  switch (t.name) {
    case "Edit":
      return diffEl(str(a.old_string), str(a.new_string));
    case "MultiEdit": {
      const wrap = document.createElement("div");
      for (const e of (Array.isArray(a.edits) ? a.edits : []))
        wrap.appendChild(diffEl(str(e.old_string), str(e.new_string)));
      return wrap;
    }
    case "Write":
      return diffEl("", str(a.content));
    case "NotebookEdit":
      return diffEl("", str(a.new_source));
    case "TodoWrite":
      return todoEl(Array.isArray(a.todos) ? a.todos : []);
    case "ExitPlanMode":
    case "exit_plan_mode":
      return mdEl(str(a.plan));
    case "AskUserQuestion":
      return askEl(a);
    case "Bash": {
      const wrap = document.createElement("div");
      const cmd = document.createElement("div"); cmd.className = "bash-cmd";
      cmd.textContent = "$ " + str(a.command);
      wrap.appendChild(cmd);
      if (out) wrap.appendChild(clampText(out));
      return wrap;
    }
    default: {
      // Read/Grep/Glob/LS/Web/MCP/other: compact args, then output.
      const wrap = document.createElement("div");
      const argStr = compactArgs(a);
      if (argStr) { const p = document.createElement("div"); p.className = "tool-args"; p.textContent = argStr; wrap.appendChild(p); }
      if (out) wrap.appendChild(clampText(out));
      else if (!argStr) wrap.appendChild(clampText(t.input || ""));
      return wrap;
    }
  }
}

// LCS line diff. ponytail: O(n*m) time+space — fine for edit snippets; for very
// large blocks (>~1.5M cell product) fall back to replace-all to avoid freezing.
function lineDiff(oldText, newText) {
  const rows = [];
  if (oldText === newText) { rows.adds = 0; rows.dels = 0; return rows; }
  const A = oldText === "" ? [] : oldText.split("\n");
  const B = newText === "" ? [] : newText.split("\n");
  const n = A.length, m = B.length;
  if (n === 0 || m === 0 || n * m > 1500000) {
    for (const s of A) rows.push({ t: "-", s });
    for (const s of B) rows.push({ t: "+", s });
    rows.adds = m; rows.dels = n; return rows;
  }
  const dp = Array.from({ length: n + 1 }, () => new Int32Array(m + 1));
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--)
      dp[i][j] = A[i] === B[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
  let i = 0, j = 0, adds = 0, dels = 0;
  while (i < n && j < m) {
    if (A[i] === B[j]) { rows.push({ t: " ", s: A[i] }); i++; j++; }
    else if (dp[i + 1][j] >= dp[i][j + 1]) { rows.push({ t: "-", s: A[i] }); i++; dels++; }
    else { rows.push({ t: "+", s: B[j] }); j++; adds++; }
  }
  while (i < n) { rows.push({ t: "-", s: A[i++] }); dels++; }
  while (j < m) { rows.push({ t: "+", s: B[j++] }); adds++; }
  rows.adds = adds; rows.dels = dels;
  return rows;
}
function diffStat(rows) { return `+${rows.adds || 0} −${rows.dels || 0}`; }

function diffEl(oldText, newText) {
  const rows = lineDiff(oldText, newText);
  const wrap = document.createElement("div");
  wrap.className = "diff";
  const big = rows.length > CODE_FOLD_LINES + 2;
  if (big) { wrap.classList.add("foldable"); wrap.style.setProperty("--fold-lines", String(CODE_FOLD_LINES)); }
  const lines = document.createElement("div"); lines.className = "diff-lines";
  for (const r of rows) {
    const ln = document.createElement("div");
    ln.className = "dl " + (r.t === "+" ? "add" : r.t === "-" ? "del" : "ctx");
    const g = document.createElement("span"); g.className = "g"; g.textContent = r.t === " " ? " " : r.t;
    const c = document.createElement("span"); c.className = "c"; c.textContent = r.s;
    ln.appendChild(g); ln.appendChild(c); lines.appendChild(ln);
  }
  wrap.appendChild(lines);
  if (big) {
    const toggle = document.createElement("button"); toggle.className = "code-fold";
    const hidden = rows.length - CODE_FOLD_LINES;
    const setLabel = () => { toggle.textContent = wrap.classList.contains("expanded") ? "Show less" : `Show ${hidden} more lines`; };
    setLabel();
    toggle.addEventListener("click", (e) => { e.stopPropagation(); wrap.classList.toggle("expanded"); setLabel(); });
    wrap.appendChild(toggle);
  }
  return wrap;
}

function todoEl(todos) {
  const wrap = document.createElement("div"); wrap.className = "todos";
  const MARK = { completed: "☑", in_progress: "◐", pending: "☐" };
  for (const td of todos) {
    const row = document.createElement("div");
    row.className = "todo " + (td.status || "pending");
    const m = document.createElement("span"); m.className = "tk"; m.textContent = MARK[td.status] || "☐";
    const tx = document.createElement("span"); tx.className = "tx";
    tx.textContent = str(td.status === "in_progress" && td.activeForm ? td.activeForm : td.content);
    row.appendChild(m); row.appendChild(tx); wrap.appendChild(row);
  }
  return wrap;
}

function askText(a) { const q = (Array.isArray(a.questions) && a.questions[0]) || a; return str(q && q.question); }
function askEl(a) {
  const wrap = document.createElement("div"); wrap.className = "ask";
  const qs = Array.isArray(a.questions) ? a.questions : (a.question ? [a] : []);
  for (const q of qs) {
    const qe = document.createElement("div"); qe.className = "ask-q"; qe.textContent = str(q.question); wrap.appendChild(qe);
    for (const o of (Array.isArray(q.options) ? q.options : [])) {
      const oe = document.createElement("div"); oe.className = "ask-o";
      const lbl = document.createElement("span"); lbl.className = "ask-lbl";
      lbl.textContent = typeof o === "string" ? o : str(o.label);
      oe.appendChild(lbl);
      if (o && o.description) { const d = document.createElement("span"); d.className = "ask-desc"; d.textContent = str(o.description); oe.appendChild(d); }
      wrap.appendChild(oe);
    }
  }
  return wrap;
}

// Long text body clamped to N lines with a Show more/less toggle (Bash/Read/
// search output). Mirrors the code-card fold.
function clampText(text) {
  const lines = String(text).split("\n");
  if (lines.length <= CODE_FOLD_LINES + 2) {
    const pre = document.createElement("div"); pre.className = "card-pre"; pre.textContent = text; return pre;
  }
  const wrap = document.createElement("div");
  const pre = document.createElement("div");
  pre.className = "card-pre foldable-body";
  pre.style.setProperty("--fold-lines", String(CODE_FOLD_LINES));
  pre.textContent = text;
  const toggle = document.createElement("button"); toggle.className = "code-fold";
  const hidden = lines.length - CODE_FOLD_LINES;
  const setLabel = () => { toggle.textContent = pre.classList.contains("expanded") ? "Show less" : `Show ${hidden} more lines`; };
  setLabel();
  toggle.addEventListener("click", (e) => { e.stopPropagation(); pre.classList.toggle("expanded"); setLabel(); });
  wrap.appendChild(pre); wrap.appendChild(toggle);
  return wrap;
}

function compactArgs(a) {
  const parts = [];
  for (const [k, v] of Object.entries(a || {})) {
    if (v == null || typeof v === "object") continue;
    const s = String(v);
    if (s.length > 200) continue;
    parts.push(`${k}: ${s}`);
  }
  return parts.join("\n");
}
function hostOf(u) { try { return new URL(u).host; } catch { return str(u).slice(0, 40); } }

// =================== permission prompts (approve / deny) ===================
// The server emits a `permission_request` event when a streaming turn's tool
// needs approval (claude --permission-prompt-tool stdio). Render an approve card
// — reusing the tool view so edits show their diff — and reply with
// `permission_response`. claude prompts sequentially, so at most one is live.
let permEl = null;
function showPermission(evt) {
  removePermission();
  const pseudo = {
    name: evt.toolName || "Tool", args: evt.input || {}, status: "running", output: "",
    input: JSON.stringify(evt.input || {}, null, 2),
  };
  const sid = evt.sessionId || state.activeId;
  const reqId = evt.requestId;
  const respond = (behavior, mode) => {
    send({ op: "permission_response", sessionId: sid, requestId: reqId, behavior, input: evt.input });
    if (mode) {
      send({ op: "set_permission_mode", sessionId: sid, mode });
      const s = state.sessions.find(x => x.id === sid); if (s) s.permission_mode = mode; syncPermLabel();
    }
    removePermission();
  };
  permEl = document.createElement("div");
  permEl.className = "msg assistant perm-row";
  const body = document.createElement("div"); body.className = "body";
  const card = document.createElement("div"); card.className = "perm";
  const head = document.createElement("div"); head.className = "perm-head";
  head.textContent = `Allow ${toolMeta(pseudo).title}?`;
  const preview = toolCard(pseudo); preview.classList.add("open");
  const btns = document.createElement("div"); btns.className = "perm-btns";
  btns.appendChild(permBtn("Allow", "allow-btn", () => respond("allow")));
  const sug = (evt.suggestions || []).find(x => x && x.type === "setMode" && x.mode);
  if (sug) btns.appendChild(permBtn(`Allow + ${permLabelFor(sug.mode)}`, "allow-btn alt", () => respond("allow", sug.mode)));
  btns.appendChild(permBtn("Deny", "deny-btn", () => respond("deny")));
  card.appendChild(head); card.appendChild(preview); card.appendChild(btns);
  body.appendChild(card); permEl.appendChild(body);
  messagesEl.appendChild(permEl);
  emptyEl.classList.add("hidden");
  ensurePinned();
}
function removePermission() { if (permEl) { permEl.remove(); permEl = null; } }
function permBtn(label, cls, on) {
  const b = document.createElement("button"); b.className = "perm-b " + cls; b.textContent = label;
  b.addEventListener("click", on); return b;
}

// =================== permission-mode pill (composer) ===================
const PERM_MODES = [
  { id: "default", label: "Ask" },
  { id: "acceptEdits", label: "Auto-edit" },
  { id: "plan", label: "Plan" },
  { id: "bypassPermissions", label: "Yolo" },
];
function permLabelFor(mode) { const m = PERM_MODES.find(x => x.id === mode); return m ? m.label : (mode || "Ask"); }
function currentMode() {
  const s = state.sessions.find(x => x.id === state.activeId);
  return (s ? s.permission_mode : state.pendingMode) || "default";
}
function syncPermLabel() { /* permission mode shown in attach menu */ }
function chooseMode(mode) {
  closeMenus();
  if (state.activeId) {
    send({ op: "set_permission_mode", sessionId: state.activeId, mode });
    const s = state.sessions.find(x => x.id === state.activeId);
    if (s) s.permission_mode = mode;   // optimistic; broadcast confirms
  } else {
    state.pendingMode = mode;          // applied on the next create
  }
  syncPermLabel();
}
function initAttachMenu() {
  const btn = $("attachBtn");
  if (!btn) return;
  const menu = document.createElement("div");
  menu.className = "model-menu"; menu.id = "permMenu";
  $("composer").appendChild(menu);
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    toggleMenu("attachMenu", openAttachMenu);
  });
}
function openAttachMenu() {
  closeMenus();
  const menu = $("attachMenu");
  menu.innerHTML = "";
  const addRow = (label, onClick) => {
    const row = document.createElement("div");
    row.className = "model-item";
    row.innerHTML = `<span>${escapeHtml(label)}</span>`;
    row.addEventListener("click", (e) => { e.stopPropagation(); closeMenus(); onClick(); });
    menu.appendChild(row);
  };
  addRow(`Project · ${basename(currentCwd()) || "Local"}`, () => openLocalMenu());
  addRow(`Permissions · ${permLabelFor(currentMode())}`, () => {
    openMenu("permMenu", PERM_MODES.map(m => ({ id: m.id, label: m.label })), currentMode(), chooseMode, "");
  });
  addRow("Change server…", () => {
    if (state.ws) { state.ws.close(); state.ws = null; }
    state.connected = false;
    clearCreds();
    window.__SYNAPSE__ = null;
    showConnectOverlay();
  });
  menu.classList.add("show");
}

function ingestResult(evt) {
  const content = evt.message && Array.isArray(evt.message.content) ? evt.message.content : [];
  const tn = state.turn;
  if (!tn) return;
  for (const c of content) {
    if (c.type === "tool_result" && c.tool_use_id && tn.tools.has(c.tool_use_id)) {
      const t = tn.tools.get(c.tool_use_id);
      t.status = c.is_error ? "error" : "done";
      t.output = typeof c.content === "string" ? c.content
               : Array.isArray(c.content) ? c.content.map(x => x.text || "").join("\n") : "";
    }
  }
  renderTurn(false);
  updatePulse();
  ensurePinned();
}

function finalizeStream() {
  const tn = state.turn;
  if (tn) {
    // any tool still "running" at turn end is treated as done
    for (const t of tn.tools.values()) if (t.status === "running") t.status = "done";
    const hasAnything = tn.order.length > 0;
    if (hasAnything) {
      ensureTurnInDom();
      renderTurn(true);   // collapse work into "Worked for Xs ›"
    } else if (tn.appended) {
      tn.el.remove();     // empty turn — drop it
    }
    state.turn = null;
  }
  removePermission();
  updatePulse();
}

// =================== history backfill ===================
// Transcript has no turn_started/stopped markers; a user message starts a new
// turn, and everything until the next user message is that turn's work + reply.
function ingestHistory(events) {
  clearMessages();
  state.turn = null;
  let pendingUserTs = 0;  // timestamp of the user msg that opened the next turn
  for (const evt of events) {
    if (evt.type === "user" && evt.message) {
      const txt = contentText(evt.message.content);
      if (txt) {
        // A real user message is a turn boundary: close the previous turn first.
        finalizeStream();
        echoUser(txt, evt.message.id);
        pendingUserTs = evt.timestamp ? Date.parse(evt.timestamp) : 0;
      } else {
        // tool_result-only user frame: part of the current turn's work, NOT a
        // boundary — route it to the active turn's tools without finalizing.
        ingestResultFromUser(evt);
      }
    } else if (evt.type === "assistant" && evt.message) {
      const hadTurn = !!state.turn;
      ingestAssistant(evt);
      // seed the new turn's start from the user message that triggered it, so
      // "Worked for Xs" includes the model's initial latency.
      if (!hadTurn && state.turn && pendingUserTs) {
        state.turn.firstTs = pendingUserTs; pendingUserTs = 0;
      }
    } else if (evt.type === "stderr") {
      pushStderr(str(evt.text));
    }
  }
  finalizeStream();
}

// In transcripts, tool_result blocks arrive inside a *user* frame; route them
// to the active turn's tools so the collapsed work shows outputs.
function ingestResultFromUser(evt) {
  ingestResult({ message: evt.message });
}

function fmtElapsed(secs) {
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60), s = secs % 60;
  return s ? `${m}m ${s}s` : `${m}m`;
}

// =================== rendering primitives ===================
// Synara style: assistant blocks are full-width with no avatar; user messages
// are right-aligned compact bubbles. No avatar column at all.
function mkMsg(role) {
  const el = document.createElement("div");
  el.className = "msg " + (role === "user" ? "user" : "assistant");
  const body = document.createElement("div");
  body.className = "body";
  el.appendChild(body);
  return el;
}

function mdEl(md) {
  const d = document.createElement("div");
  d.className = "md";
  d.innerHTML = marked.parse(md);
  // Upgrade marked's bare <pre><code> into Synara code cards (lang label + Copy).
  d.querySelectorAll("pre > code").forEach((codeEl) => {
    const pre = codeEl.parentElement;
    const cls = codeEl.className || "";
    const m = cls.match(/language-([\w+-]+)/);
    const lang = m ? m[1] : "";
    const card = codeCard(lang, codeEl.textContent || "");
    pre.replaceWith(card);
  });
  return d;
}

function cardEl(kind, icon, name, sub, bodyText, status) {
  const card = document.createElement("div");
  card.className = "card";
  const head = document.createElement("div");
  head.className = "card-head";
  head.innerHTML = `<span class="ic">${icon}</span>` +
    `<span class="nm">${escapeHtml(name)}${sub ? `<span class="sub">${escapeHtml(sub)}</span>` : ""}</span>` +
    (status !== undefined ? `<span class="st ${status}"></span>` : "") +
    `<span class="chev">▸</span>`;
  const body = document.createElement("div");
  body.className = "card-body";
  const txt = bodyText || "";
  const lines = txt.split("\n");
  if (lines.length > CODE_FOLD_LINES + 2) {
    // long tool output: clamp + "Show more" inside the (already expandable) card
    const pre = document.createElement("div");
    pre.className = "card-pre foldable-body";
    pre.style.setProperty("--fold-lines", String(CODE_FOLD_LINES));
    pre.textContent = txt;
    const toggle = document.createElement("button");
    toggle.className = "code-fold";
    const hidden = lines.length - CODE_FOLD_LINES;
    const setLabel = () => {
      toggle.textContent = pre.classList.contains("expanded")
        ? "Show less" : `Show ${hidden} more lines`;
    };
    setLabel();
    toggle.addEventListener("click", () => { pre.classList.toggle("expanded"); setLabel(); });
    body.appendChild(pre); body.appendChild(toggle);
  } else {
    body.textContent = txt;
  }
  card.appendChild(head); card.appendChild(body);
  head.addEventListener("click", () => card.classList.toggle("open"));
  return card;
}

// Lines beyond this collapse behind a "Show N more lines" toggle.
const CODE_FOLD_LINES = 14;

function codeCard(lang, code, idx) {
  const card = document.createElement("div");
  card.className = "code-card";
  const head = document.createElement("div");
  head.className = "code-head";
  const copy = document.createElement("button");
  copy.className = "copy"; copy.textContent = "Copy";
  copy.addEventListener("click", () => {
    copyText(code);
    copy.classList.add("copied"); copy.textContent = "✓ Copied";
    setTimeout(() => { copy.classList.remove("copied"); copy.textContent = "Copy"; }, 1200);
  });
  head.innerHTML = `<span class="lang">${escapeHtml(lang || "code")}</span>`;
  head.appendChild(copy);
  const pre = document.createElement("pre");
  const codeEl = document.createElement("code");
  codeEl.className = lang ? `language-${lang}` : "";
  codeEl.textContent = code;
  pre.appendChild(codeEl);
  try { hljs.highlightElement(codeEl); } catch {}
  card.appendChild(head); card.appendChild(pre);

  // Fold long code: clamp to N lines, reveal the rest with a toggle.
  const total = code.split("\n").length;
  if (total > CODE_FOLD_LINES + 2) {
    card.classList.add("foldable");
    pre.style.setProperty("--fold-lines", String(CODE_FOLD_LINES));
    const hidden = total - CODE_FOLD_LINES;
    const toggle = document.createElement("button");
    toggle.className = "code-fold";
    const setLabel = () => {
      toggle.textContent = card.classList.contains("expanded")
        ? "Show less" : `Show ${hidden} more lines`;
    };
    setLabel();
    toggle.addEventListener("click", () => {
      card.classList.toggle("expanded");
      setLabel();
    });
    card.appendChild(toggle);
  }
  return card;
}

function pushError(text) {
  const el = mkMsg("assistant");
  const body = el.querySelector(".body");
  const e = document.createElement("div");
  e.className = "err-card";
  e.innerHTML = `<span class="ic">⚠</span><span>${escapeHtml(text)}</span>`;
  body.appendChild(e);
  addBlock(el);
  ensurePinned();
}

function pushStderr(text) {
  const el = mkMsg("assistant");
  const body = el.querySelector(".body");
  const e = document.createElement("div");
  e.className = "err-card";
  e.innerHTML = `<span class="ic">⚠</span><span>${escapeHtml(text)}</span>`;
  body.appendChild(e);
  addBlock(el);
  ensurePinned();
}

function echoUser(text, mid) {
  // dedupe: don't re-add a user turn we already echoed for this mid
  if (mid) {
    const existing = state.blocks.find(b => b.role === "user" && b.mid === mid);
    if (existing) return;
  }
  const el = document.createElement("div");
  el.className = "msg user";
  const bubble = document.createElement("div");
  bubble.className = "bubble";
  bubble.textContent = text;
  el.appendChild(bubble);
  state.blocks.push({ el, role: "user", mid });
  messagesEl.appendChild(el);
  emptyEl.classList.add("hidden");
  ensurePinned();
}

function addBlock(el) {
  state.blocks.push({ el });
  messagesEl.appendChild(el);
  emptyEl.classList.add("hidden");
}

function clearMessages() {
  messagesEl.innerHTML = "";
  state.blocks = [];
  permEl = null;
  emptyEl.classList.remove("hidden");
}

// =================== smart scroll ===================
// `pinned` tracks whether the user is following the bottom. It only flips off
// when the user scrolls UP themselves — streamed content never unpins it. This
// fixes the "screen doesn't follow output" bug: measuring nearBottom() after
// appending content always read false once a turn grew past the threshold.
let pinned = true;
const NEAR_BOTTOM_PX = 80;
function nearBottom() {
  return scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < NEAR_BOTTOM_PX;
}
function scrollToBottom() {
  scroller.scrollTop = scroller.scrollHeight;
}
// Run a DOM mutation, then keep the bottom pinned if we were already following.
function ensurePinned() {
  if (pinned) {
    // double rAF: let layout settle (markdown/code height) before pinning
    requestAnimationFrame(() => { scrollToBottom(); requestAnimationFrame(scrollToBottom); });
    $("newPill").classList.remove("show");
  } else {
    $("newPill").classList.add("show");
  }
}
scroller.addEventListener("scroll", () => {
  // User-driven scroll decides pin state.
  pinned = nearBottom();
  if (pinned) $("newPill").classList.remove("show");
});
$("newPill").addEventListener("click", () => {
  pinned = true;
  scrollToBottom();
  $("newPill").classList.remove("show");
});

// =================== busy / title ===================
function setBusy(b) {
  state.busy = b;
  sendBtn.classList.toggle("busy", b);
  updatePulse();
  updateSend();
}
// Activity indicator: keep the turn's activity feed visible while working.
let tickTimer = null;
function updatePulse() {
  const tn = state.turn;
  const toolRunning = tn && [...tn.tools.values()].some(t => t.status === "running");
  const last = tn && tn.order[tn.order.length - 1];
  const hasReply = !!(last && last.kind === "text" && last.text);
  const show = state.busy && (toolRunning || !hasReply);
  if (show && tn) {
    ensureTurnInDom();
    renderTurn(false);
    ensurePinned();
  }
  if (state.busy && !tickTimer) {
    tickTimer = setInterval(() => {
      if (state.turn) {
        state.turn.ticks++;
        renderTurn(false);
        if (state.turn.activityFeed) {
          const scroll = state.turn.activityFeed.querySelector(".activity-scroll");
          if (scroll) scrollActivityFeed(state.turn.activityFeed, scroll);
        }
        ensurePinned();
      }
    }, 1000);
  } else if (!state.busy && tickTimer) {
    clearInterval(tickTimer); tickTimer = null;
  }
}

// =================== sessions ===================
function setSessions(list) {
  state.sessions = list || [];
  renderSessions();
  // After a server restart, auto-created sessions come back with new ids, so a
  // still-selected old id is now dead — requesting its history returns found:false
  // and the view stays stuck on the welcome page. Drop the dead id and fall through
  // to auto-select a live session instead of sitting on "new session" forever.
  if (state.activeId && !state.sessions.some((s) => s.id === state.activeId)) {
    state.activeId = "";
    if (state.view === "chat") showSessions();
  }
  updateTopbar();
}
function upsertSession(s) {
  const i = state.sessions.findIndex(x => x.id === s.id);
  if (i >= 0) state.sessions[i] = s; else state.sessions.unshift(s);
  renderSessions();
}
// Track a session's running state from live turn_started/turn_stopped (broadcast
// for every session) so the drawer dot and busy-on-open stay correct even for
// turns started on another device.
function setSessionState(id, st) {
  const ses = state.sessions.find(x => x.id === id);
  if (!ses || ses.state === st) return;
  ses.state = st;
  renderSessions();
  if (id === state.activeId && state.view === "chat") updateTopbar();
}
// Session titles come from the transcript's first user line, which is often
// command/hook boilerplate (/goal stop-hooks, continuation summaries, local-
// command caveats) rather than a real prompt. Strip the known noise so the
// drawer + title bar read as what the session is actually about.
function cleanTitle(raw) {
  let t = (raw || "").trim();
  if (!t) return "New session";
  // /goal & stop-hook lines: the meaningful bit is the quoted condition. The
  // server truncates the title, so the closing quote may be gone — capture to
  // end and drop a trailing quote if present.
  const goal = t.match(/Stop hook is now active with condition:\s*["“](.+)/i);
  if (goal) t = goal[1].replace(/["”]\s*$/, "");
  t = t
    .replace(/<\/?[a-z][a-z-]*(?:\s[^>]*)?>/gi, " ")  // <command-name> … wrappers
    .replace(/^\s*Caveat:.*?explicitly requested\.?\s*/is, "")
    .replace(/^This session is being continued from a previous conversation.*$/is, "Continued session")
    .replace(/^\/(goal|compact|clear|model|ponytail)\b[: ]*/i, "")  // leading slash command
    .replace(/\s+/g, " ")
    .trim();
  return t || "New session";
}
// Per-row actions (⋯): rename / delete. A tiny popup anchored to the button.
function closeRowMenu() { const m = $("rowMenu"); if (m) m.remove(); }
function rowMenu(s, anchor) {
  closeRowMenu();
  const m = document.createElement("div"); m.className = "row-menu"; m.id = "rowMenu";
  const rename = document.createElement("div"); rename.className = "row-mi"; rename.textContent = "Rename";
  rename.addEventListener("click", (e) => {
    e.stopPropagation(); closeRowMenu();
    const n = prompt("Rename session", cleanTitle(s.name) || "");
    if (n && n.trim()) send({ op: "rename", sessionId: s.id, name: n.trim() });
  });
  const del = document.createElement("div"); del.className = "row-mi danger"; del.textContent = "Delete";
  del.addEventListener("click", (e) => {
    e.stopPropagation(); closeRowMenu();
    if (confirm("Remove this session from the list?")) send({ op: "delete", sessionId: s.id });
  });
  m.appendChild(rename); m.appendChild(del);
  document.body.appendChild(m);
  const r = anchor.getBoundingClientRect();
  m.style.top = `${r.bottom + 4}px`;
  m.style.left = `${Math.max(8, Math.min(r.right - 150, window.innerWidth - 158))}px`;
}
document.addEventListener("click", closeRowMenu);

function workspaceName() {
  const sorted = [...state.sessions].sort((a, b) => (b.started_at || 0) - (a.started_at || 0));
  const cwd = sorted[0] && sorted[0].cwd;
  return cwd ? basename(cwd) : "Synapse";
}

function dayGroup(ms) {
  if (!ms) return "Earlier";
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const yesterday = today - 86400000;
  const d = new Date(ms);
  const day = new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  if (day >= today) return "Today";
  if (day >= yesterday) return "Yesterday";
  return "Earlier";
}

function sessionSubtitle(s) {
  if (s.state === "busy") return { text: "Working", cls: "working" };
  if (s.state === "error") return { text: "Failed", cls: "error" };
  const t = s.started_at || 0;
  return t ? { text: relTime(t), cls: "" } : { text: "", cls: "" };
}

function showSessions() {
  state.view = "sessions";
  document.body.classList.add("mode-sessions");
  document.body.classList.remove("mode-chat");
  updateTopbar();
  renderSessions();
}

function showChat() {
  state.view = "chat";
  document.body.classList.remove("mode-sessions");
  document.body.classList.add("mode-chat");
  updateTopbar();
}

function updateTopbar() {
  if (state.view === "sessions") {
    titleName.textContent = workspaceName();
  } else {
    const s = state.sessions.find(x => x.id === state.activeId);
    titleName.textContent = s ? cleanTitle(s.name) : "Chat";
  }
}

function renderSessions() {
  const q = ($("search").value || "").toLowerCase();
  const list = $("sessionList");
  list.innerHTML = "";
  const f = state.sessions
    .filter(s => !q || (s.name || "").toLowerCase().includes(q) || (s.cwd || "").toLowerCase().includes(q))
    .sort((a, b) => (b.started_at || 0) - (a.started_at || 0));

  if (!f.length) {
    const hint = document.createElement("div");
    hint.className = "empty-hint";
    hint.textContent = q ? "No matching sessions" : "No sessions yet — tap + to start";
    list.appendChild(hint);
    return;
  }

  const groups = new Map();
  const order = ["Today", "Yesterday", "Earlier"];
  for (const s of f) {
    const g = dayGroup(s.started_at || 0);
    if (!groups.has(g)) groups.set(g, []);
    groups.get(g).push(s);
  }

  for (const gName of order) {
    const items = groups.get(gName);
    if (!items || !items.length) continue;
    const head = document.createElement("div");
    head.className = "s-time";
    head.textContent = gName;
    list.appendChild(head);
    for (const s of items) {
      const row = document.createElement("div");
      row.className = "sess-row" + (s.id === state.activeId ? " active" : "");
      const sub = sessionSubtitle(s);
      const subHtml = sub.text
        ? `<div class="sess-sub ${sub.cls}">${escapeHtml(sub.text)}</div>` : "";
      row.innerHTML =
        `<div class="sess-icon">${s.state === "busy" ? "◐" : "+"}</div>` +
        `<div class="sess-body">` +
          `<div class="sess-title">${escapeHtml(cleanTitle(s.name))}</div>` +
          subHtml +
        `</div>` +
        `<button class="sess-more" aria-label="Session actions">⋯</button>`;
      row.addEventListener("click", () => select(s.id));
      row.querySelector(".sess-more").addEventListener("click", (e) => {
        e.stopPropagation(); rowMenu(s, e.currentTarget);
      });
      list.appendChild(row);
    }
  }
}

// Relative time for the session list. Runs in the browser, so Date is available
// (the workflow-sandbox caveat elsewhere doesn't apply).
function relTime(ms) {
  if (!ms) return "";
  const s = Math.floor((Date.now() - ms) / 1000);
  if (s < 60) return "just now";
  const m = Math.floor(s / 60); if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60); if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24); if (d < 7) return `${d}d ago`;
  return new Date(ms).toLocaleDateString();
}
function select(id) {
  state.activeId = id;
  const s = state.sessions.find(x => x.id === id);
  if (s) {
    titleName.textContent = cleanTitle(s.name);
    syncModelLabel(); syncLocalLabel(); syncPermLabel();
    const es = $("emptySub"); if (es) es.textContent = basename(s.cwd);
  }
  clearMessages();
  state.turn = null;
  // Seed busy from the session's current state so opening a session whose turn
  // is already running (started on another device) shows the replying status at
  // once; the live turn_stopped clears it.
  setBusy(s ? s.state === "busy" : false);
  send({ op: "history", sessionId: id, limit: 400 });
  showChat();
}

// =================== composer ===================
function autoGrow() {
  inputEl.style.height = "auto";
  inputEl.style.height = Math.min(inputEl.scrollHeight, 160) + "px";
  updateSend();
}
function updateSend() {
  const has = inputEl.value.trim().length > 0;
  sendBtn.classList.remove("active", "busy");
  if (state.busy) sendBtn.classList.add("busy");
  else if (has) sendBtn.classList.add("active");
}
inputEl.addEventListener("input", autoGrow);
inputEl.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey && window.__SYNAPSE__) {
    // mobile uses the button; on desktop Enter sends
    e.preventDefault(); doSend();
  }
});
sendBtn.addEventListener("click", () => {
  if (state.busy) { send({ op: "stop", sessionId: state.activeId }); return; }
  doSend();
});
function doSend() {
  const text = inputEl.value.trim();
  if (!text) return;
  inputEl.value = ""; autoGrow();
  if (!state.activeId) {
    // No session yet: start one (in the selected model/repo) and send the
    // message into it once it's created. Type-and-send always works.
    state.pendingSend = text;
    newSession();
    return;
  }
  // No optimistic echo: the server broadcasts the user message to every device
  // viewing this session and we render it from that broadcast (handleEvent
  // "user"), so all devices show an identical transcript.
  send({ op: "send", sessionId: state.activeId, content: text });
}

// =================== navigation ===================
$("backBtn").addEventListener("click", showSessions);
$("newBtn").addEventListener("click", newSession);
$("refreshBtn").addEventListener("click", () => send({ op: "refresh" }));
$("searchBtn").addEventListener("click", () => {
  const wrap = $("searchWrap");
  const hidden = wrap.classList.toggle("hidden");
  if (!hidden) $("search").focus();
  else { $("search").value = ""; renderSessions(); }
});
$("search").addEventListener("input", renderSessions);

function newSession() {
  const opts = {};
  if (state.pendingModel) opts.model = state.pendingModel;
  if (state.pendingCwd) opts.cwd = state.pendingCwd;
  if (state.pendingMode) opts.permission_mode = state.pendingMode;
  send({ op: "create", opts });
}

// suggestions
document.querySelectorAll("#empty .suggestions button").forEach(b => {
  b.addEventListener("click", () => {
    inputEl.value = b.dataset.prompt; autoGrow(); inputEl.focus();
  });
});

// =================== composer pickers (model + project) ===================
// The "◆ Model" / "⎇ Local" pills open popovers of the server catalogs.
//   model:   pick on an active session → `set_model` (next turn); with no
//            session it's remembered for the next `create`.
//   project: pick a git repo → start a fresh session there.
function basename(p) {
  const a = (p || "").split("/").filter(Boolean);
  return a[a.length - 1] || p || "";
}
function labelForModel(id) {
  if (!id) return "Default";
  const m = state.models.find(x => x.id === id);
  return m ? m.label : id;
}
function currentModelId() {
  const s = state.sessions.find(x => x.id === state.activeId);
  return (s ? s.model : (state.pendingModel || state.defaultModel)) || "";
}
function currentCwd() {
  const s = state.sessions.find(x => x.id === state.activeId);
  return (s ? s.cwd : state.pendingCwd) || "";
}
function syncModelLabel() {
  const ml = $("modelLabel");
  const id = currentModelId();
  if (ml) ml.textContent = id ? labelForModel(id) : "Auto";
}
function syncLocalLabel() {
  const ll = $("localLabel"); const c = currentCwd();
  if (ll) ll.textContent = c ? basename(c) : "Local";
}
function closeMenus() {
  $("modelMenu").classList.remove("show");
  $("localMenu").classList.remove("show");
  const am = $("attachMenu"); if (am) am.classList.remove("show");
  const pm = $("permMenu"); if (pm) pm.classList.remove("show");
}
// Render `items` ([{id,label,title?}]) into menu `id`, marking `cur` selected.
function openMenu(id, items, cur, onPick, emptyMsg) {
  closeMenus();
  if (!items.length) { toast(emptyMsg); return; }
  const menu = $(id);
  menu.innerHTML = "";
  for (const it of items) {
    const row = document.createElement("div");
    row.className = "model-item" + (it.id === cur ? " sel" : "");
    row.innerHTML = `<span>${escapeHtml(it.label)}</span>`;
    if (it.title) row.title = it.title;
    row.addEventListener("click", (e) => { e.stopPropagation(); onPick(it.id); });
    menu.appendChild(row);
  }
  menu.classList.add("show");
  const sel = menu.querySelector(".model-item.sel");
  if (sel) sel.scrollIntoView({ block: "nearest" });
}
function toggleMenu(id, build) {
  const open = $(id).classList.contains("show");
  closeMenus();
  if (!open) build();
}
function chooseModel(id) {
  closeMenus();
  if (state.activeId) {
    send({ op: "set_model", sessionId: state.activeId, model: id });
    const s = state.sessions.find(x => x.id === state.activeId);
    if (s) s.model = id;                 // optimistic; the broadcast confirms
  } else {
    state.pendingModel = id;
  }
  syncModelLabel();
}
function chooseCwd(path) {
  closeMenus();
  state.pendingCwd = path;               // newSession() reads this
  syncLocalLabel();
  newSession();                          // selecting a repo starts a session there
}
$("modelCtl").addEventListener("click", (e) => {
  e.stopPropagation();
  toggleMenu("modelMenu", () => openMenu("modelMenu",
    state.models.map(m => ({ id: m.id, label: m.label })),
    currentModelId(), chooseModel, "No models configured"));
});
// The project menu always offers a free-text path (to start a session in a repo
// not in the discovered list), then the discovered git repos. openMenu() bails
// with a toast on an empty list — which would hide the input — so this one is
// bespoke. The input and the rows both lead to chooseCwd(), same as a pick.
function openLocalMenu() {
  closeMenus();
  const menu = $("localMenu");
  menu.innerHTML = "";
  const inp = document.createElement("input");
  inp.className = "path-input";
  inp.placeholder = "输入路径，如 ~/code/foo";
  inp.addEventListener("click", (e) => e.stopPropagation());
  inp.addEventListener("keydown", (e) => {
    if (e.key === "Enter") { const p = inp.value.trim(); if (p) chooseCwd(p); }
  });
  menu.appendChild(inp);
  const cur = currentCwd();
  for (const p of state.cwds) {
    const row = document.createElement("div");
    row.className = "model-item" + (p === cur ? " sel" : "");
    row.innerHTML = `<span>${escapeHtml(basename(p))}</span>`;
    row.title = p;
    row.addEventListener("click", (e) => { e.stopPropagation(); chooseCwd(p); });
    menu.appendChild(row);
  }
  menu.classList.add("show");
  const sel = menu.querySelector(".model-item.sel");
  if (sel) sel.scrollIntoView({ block: "nearest" });
  inp.focus();
}
// local picker opened from attach (+) menu
document.addEventListener("click", closeMenus);

// =================== toast ===================
let toastTimer = null;
function toast(msg) {
  $("toastText").textContent = msg;
  $("toast").classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => $("toast").classList.remove("show"), 3500);
}
$("toastClose").addEventListener("click", () => $("toast").classList.remove("show"));

// =================== clipboard ===================
// Native bridge: if window.__synapseCopy__ exists (Rust), use it; else fallback.
function copyText(text) {
  if (window.__synapseCopy__) { window.__synapseCopy__(text); return; }
  try { navigator.clipboard.writeText(text); } catch {}
}

// =================== helpers ===================
function str(v) { return v == null ? "" : String(v); }
function escapeHtml(s) {
  return str(s).replace(/[&<>"']/g, c => ({ "&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;" }[c]));
}
function contentText(content) {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) return content.filter(c => c.type === "text").map(c => c.text).join("");
  return "";
}
function firstLine(s) { return str(s).split("\n")[0].slice(0, 80); }

// =================== boot ===================
initAttachMenu();
$("sheetClose").addEventListener("click", closeSheet);
$("sheetMask").addEventListener("click", closeSheet);
$("pairConnect").addEventListener("click", pairFromForm);
$("pairManualConnect").addEventListener("click", pairFromForm);
window.__synapse = { handle, handleEvent, state, parsePairLink, applyCreds };
connect();
})();
