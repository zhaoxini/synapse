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
const subText = $("subText");
const dot = $("dot");

const state = {
  ws: null,
  url: "",
  backoff: 1000,
  connected: false,
  busy: false,
  activeId: "",
  sessions: [],
  blocks: [],         // rendered message elements (for empty/clear bookkeeping)
  // The current assistant turn. Synara model: a turn's thinking + tool calls are
  // "work"; while running they show live, and once the turn settles they collapse
  // into a single "Worked for Xs ›" disclosure above the final reply text.
  //   { el, workWrap, workBody, replyWrap, items:[], tools:Map, text, startMs }
  turn: null,
  msNow: 0,           // monotonic-ish clock fed from frames (no Date in workflow ctx, fine here)
};

// ---- credential injection from native (pairing) ----
function creds() {
  if (window.__SYNAPSE__) return window.__SYNAPSE__;
  // dev fallback from querystring
  const p = new URLSearchParams(location.search);
  const h = p.get("host"), port = p.get("port"), tok = p.get("token");
  if (h && tok) return { host: h, port: port || "4173", token: tok, tls: p.get("tls") === "1", path: p.get("path") || "" };
  return null;
}

function buildUrl(c) {
  const scheme = c.tls ? "wss" : "ws";
  if (c.path) return `${scheme}://${c.host}:${c.port}${c.path}?token=${c.token}`;
  return `${scheme}://${c.host}:${c.port}/?token=${c.token}`;
}

// =================== connection ===================
function connect() {
  const c = creds();
  if (!c) { toast("No pairing credentials"); return; }
  state.url = buildUrl(c);
  doConnect(true);
}

function doConnect(first) {
  try { state.ws = new WebSocket(state.url); }
  catch (e) { scheduleReconnect(first); return; }
  state.ws.onopen = () => {
    state.connected = true;
    state.backoff = 1000;
    $("reconnect").classList.remove("show");
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
      // pairing failure — surface, no retry loop (native will re-inject on retry)
      toast("Could not connect");
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
    case "hello": setSessions(v.sessions || []); break;
    case "sessions": setSessions(v.sessions || []); break;
    case "created":
      setSessions(state.sessions); // list update follows via event
      select(v.session.id);
      break;
    case "sessions": break;
    case "history":
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
  if (typeof evt.ttft_ms === "number") state.msNow += 0; // (kept simple; elapsed uses counters below)
  if (t === "system") {
    const sub = evt.subtype;
    if (sub === "session_created") { upsertSession(evt.session); }
    else if (sub === "turn_started") { startTurn(); setBusy(true); }
    else if (sub === "turn_stopped") { setBusy(false); finalizeStream(); }
    else if (sub === "bridge_error") { pushError(str(evt.error) || "Turn failed"); setBusy(false); finalizeStream(); }
    else if (sub === "fallback_to_json") { /* no-op */ }
    return;
  }
  if (t === "assistant") { ingestAssistant(evt); return; }
  if (t === "user") {
    if (evt.message) {
      const txt = contentText(evt.message.content);
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
    order: [],            // ordered work items: {kind:'thinking'|'tool', ...}
    thinking: "",
    text: "",
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
      tn.text += blk.text;
    } else if (blk.type === "thinking" && typeof blk.thinking === "string" && blk.thinking) {
      tn.thinking += blk.thinking;
    } else if (blk.type === "tool_use") {
      const tool = {
        id: blk.id, name: blk.name,
        input: typeof blk.input === "string" ? blk.input : JSON.stringify(blk.input ?? {}, null, 2),
        status: "running", output: "",
      };
      tn.tools.set(blk.id, tool);
      tn.order.push({ kind: "tool", id: blk.id });
    }
  }

  const hasContent = tn.text || tn.thinking || tn.tools.size > 0;
  if (hasContent) { ensureTurnInDom(); renderTurn(false); }
  updatePulse();
  ensurePinned();
}

// Render the active turn. `settled` collapses the work region into a
// "Worked for Xs ›" disclosure (Synara style); while running it shows live.
function renderTurn(settled) {
  const tn = state.turn;
  if (!tn) return;
  const hasWork = tn.thinking || tn.tools.size > 0;

  // ----- work region -----
  tn.workWrap.innerHTML = "";
  if (hasWork) {
    if (settled) {
      // collapsed: one "Worked for Xs ›" row + hairline; expand to reveal items
      const wrap = document.createElement("div"); wrap.className = "worked";
      const trig = document.createElement("button"); trig.className = "worked-trig";
      // elapsed: live timer (ticks) if present, else frame-timestamp span (history)
      const tsSecs = tn.lastTs > tn.firstTs ? Math.round((tn.lastTs - tn.firstTs) / 1000) : 0;
      const secs = tn.ticks > 0 ? tn.ticks : tsSecs;
      const label = secs > 0 ? `Worked for ${fmtElapsed(secs)}` : "Details";
      trig.innerHTML = `<span>${label}</span><span class="chev">▸</span>`;
      const panel = document.createElement("div"); panel.className = "worked-panel";
      buildWorkItems(panel, tn);
      trig.addEventListener("click", () => {
        wrap.classList.toggle("open");
      });
      wrap.appendChild(trig); wrap.appendChild(panel);
      tn.workWrap.appendChild(wrap);
      const hr = document.createElement("div"); hr.className = "hr";
      tn.workWrap.appendChild(hr);
    } else {
      // live: show work items inline (thinking card + running tool cards)
      buildWorkItems(tn.workWrap, tn);
    }
  }

  // ----- reply region -----
  tn.replyWrap.innerHTML = "";
  if (tn.text) tn.replyWrap.appendChild(mdEl(tn.text));
}

function buildWorkItems(container, tn) {
  if (tn.thinking) {
    container.appendChild(cardEl("thinking", "✦", "Thinking", null, tn.thinking));
  }
  for (const it of tn.order) {
    if (it.kind === "tool") {
      const t = tn.tools.get(it.id);
      if (!t) continue;
      const sub = t.name === "Bash" ? firstLine(t.input) : "";
      const bodyText = t.output ? `${t.input}\n\n${t.output}` : t.input;
      container.appendChild(cardEl("tool", "✦", t.name, sub, bodyText, t.status));
    }
  }
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
    const hasWork = tn.thinking || tn.tools.size > 0;
    const hasAnything = hasWork || tn.text;
    if (hasAnything) {
      ensureTurnInDom();
      renderTurn(true);   // collapse work into "Worked for Xs ›"
    } else if (tn.appended) {
      tn.el.remove();     // empty turn — drop it
    }
    state.turn = null;
  }
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
  sendBtn.textContent = b ? "■" : "↑";
  dot.classList.toggle("busy", b);
  updatePulse();
}
// The typing indicator shows whenever the turn is busy and no assistant TEXT is
// streaming yet — before the first token, while thinking, and while a tool runs.
// It sits at the end of the list (after any live work items).
let pulseEl = null;
let tickTimer = null;
function updatePulse() {
  const show = state.busy && !(state.turn && state.turn.text);
  if (show) {
    if (!pulseEl) {
      pulseEl = document.createElement("div");
      pulseEl.className = "msg assistant pulse-row";
      pulseEl.innerHTML = `<div class="body"><div class="pulse"><i></i><i></i><i></i></div></div>`;
    }
    messagesEl.appendChild(pulseEl); // move to end (after work items)
    emptyEl.classList.add("hidden");
    ensurePinned();
  } else if (pulseEl) {
    pulseEl.remove(); pulseEl = null;
  }
  // elapsed counter: tick once a second while busy, feeding "Worked for Xs"
  if (state.busy && !tickTimer) {
    tickTimer = setInterval(() => { if (state.turn) state.turn.ticks++; }, 1000);
  } else if (!state.busy && tickTimer) {
    clearInterval(tickTimer); tickTimer = null;
  }
}

// =================== sessions ===================
function setSessions(list) {
  state.sessions = list || [];
  renderSessions();
  if (!state.activeId && state.sessions.length) {
    select(state.sessions[0].id);
  }
}
function upsertSession(s) {
  const i = state.sessions.findIndex(x => x.id === s.id);
  if (i >= 0) state.sessions[i] = s; else state.sessions.unshift(s);
  renderSessions();
}
function renderSessions() {
  const q = $("search").value.toLowerCase();
  const list = $("sessionList");
  list.innerHTML = "";
  const f = state.sessions.filter(s =>
    !q || (s.name || "").toLowerCase().includes(q) || (s.cwd || "").toLowerCase().includes(q));
  for (const s of f) {
    const it = document.createElement("div");
    it.className = "s-item" + (s.id === state.activeId ? " active" : "");
    const stCls = s.state === "busy" ? " busy" : (s.state === "error" ? " error" : "");
    const dir = (s.cwd || "").split("/").filter(Boolean).pop() || s.cwd || "";
    it.innerHTML =
      `<div class="nm"><span class="st${stCls}"></span><span class="label">${escapeHtml(s.name || "Session")}</span></div>` +
      `<div class="meta">${escapeHtml(dir)}${s.model ? " · " + escapeHtml(s.model) : ""}</div>`;
    it.addEventListener("click", () => select(s.id));
    list.appendChild(it);
  }
}
function select(id) {
  state.activeId = id;
  const s = state.sessions.find(x => x.id === id);
  if (s) {
    titleName.textContent = s.name || "Session";
    subText.textContent = s.model || s.cwd || "session";
    dot.className = s.state === "busy" ? "busy" : (s.state === "error" ? "error" : "");
    // composer controls + empty-state subtitle reflect the active session
    const dir = (s.cwd || "").split("/").filter(Boolean).pop() || "";
    const ml = $("modelLabel"); if (ml) ml.textContent = s.model || "Model";
    const ll = $("localLabel"); if (ll && dir) ll.textContent = dir;
    const es = $("emptySub"); if (es) es.textContent = dir || "";
  }
  clearMessages();
  state.turn = null;
  send({ op: "history", sessionId: id, limit: 400 });
  closeDrawer();
}

// =================== composer ===================
function autoGrow() {
  inputEl.style.height = "auto";
  inputEl.style.height = Math.min(inputEl.scrollHeight, 140) + "px";
  updateSend();
}
function updateSend() {
  const has = inputEl.value.trim().length > 0;
  if (state.busy) { sendBtn.className = "busy"; sendBtn.textContent = "■"; }
  else { sendBtn.className = has ? "active" : ""; sendBtn.textContent = "↑"; }
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
  if (!state.activeId) { toast("No session"); return; }
  echoUser(text);
  send({ op: "send", sessionId: state.activeId, content: text });
  inputEl.value = ""; autoGrow();
}

// =================== drawer ===================
$("drawerBtn").addEventListener("click", openDrawer);
$("drawerClose").addEventListener("click", closeDrawer);
$("drawerMask").addEventListener("click", closeDrawer);
$("newBtn").addEventListener("click", newSession);
$("newSessionBtn").addEventListener("click", newSession);
$("refreshBtn").addEventListener("click", () => send({ op: "refresh" }));
$("search").addEventListener("input", renderSessions);
function openDrawer() { $("drawer").classList.add("show"); $("drawerMask").classList.add("show"); }
function closeDrawer() { $("drawer").classList.remove("show"); $("drawerMask").classList.remove("show"); }
function newSession() {
  send({ op: "create", opts: {} });
  closeDrawer();
}

// suggestions
document.querySelectorAll("#empty .suggestions button").forEach(b => {
  b.addEventListener("click", () => {
    inputEl.value = b.dataset.prompt; autoGrow(); inputEl.focus();
  });
});

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
// Dev/debug hooks: expose the inbound dispatcher + state for inspection. Harmless
// in production; lets tooling drive synthetic frames without a live API.
window.__synapse = { handle, handleEvent, state };
connect();
})();
