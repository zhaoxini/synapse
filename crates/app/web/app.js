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
  // rendered messages: blocks keyed by stable identity
  // each: {el, kind, role, mid, text, toolName, toolStatus, expanded, codeLang}
  blocks: [],
  // accumulate assistant stream by message.id
  streamBuf: new Map(), // mid -> {text, tools:Map, thinking:[]}
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
  if (t === "system") {
    const sub = evt.subtype;
    if (sub === "session_created") { upsertSession(evt.session); }
    else if (sub === "turn_started") { setBusy(true); }
    else if (sub === "turn_stopped") { setBusy(false); finalizeStream(); }
    else if (sub === "bridge_error") { pushError(str(evt.error) || "Turn failed"); setBusy(false); }
    else if (sub === "fallback_to_json") { /* no-op */ }
    return;
  }
  if (t === "assistant") { ingestAssistant(evt); return; }
  if (t === "user") {
    // echoed user turn (and live local echo handled on send)
    if (evt.message) {
      const txt = contentText(evt.message.content);
      if (txt) echoUser(txt, evt.message.id);
    }
    return;
  }
  if (t === "result") {
    // tool results / final result: attach to last tool card if present
    ingestResult(evt);
    return;
  }
  if (t === "stderr") {
    const txt = str(evt.text);
    if (txt) pushStderr(txt);
    return;
  }
}

// =================== assistant stream assembly ===================
function ingestAssistant(evt) {
  const msg = evt.message;
  if (!msg) return;
  const content = Array.isArray(msg.content)
    ? msg.content
    : (typeof msg.content === "string" ? [{ type: "text", text: msg.content }] : []);

  // A genuine auth/quota error frame is a synthetic assistant turn: it carries
  // an explicit error marker and a single text block, with no real message id.
  // (History transcript frames also lack message.id but are normal replies, so
  // "no id" alone must NOT mean error — that was the bug that turned backfilled
  // assistant messages into red error cards.)
  const isError = (evt.error || msg.error) && content.length && content[0].text;
  if (isError) {
    pushError(str(content[0].text));
    return;
  }

  // Key the assembly buffer by message.id when present; fall back to the frame
  // uuid (history transcript) so each turn still gets its own shell.
  const mid = str(msg.id) || str(evt.uuid) || ("m" + state.blocks.length);

  let buf = state.streamBuf.get(mid);
  if (!buf) {
    buf = { text: "", tools: new Map(), order: [], el: null };
    state.streamBuf.set(mid, buf);
    // real content arrived — drop the typing indicator and open a message shell
    showPulse(false);
    buf.el = mkMsg("assistant");
    addBlock(buf.el);
  }

  for (const blk of content) {
    if (blk.type === "text" && typeof blk.text === "string") {
      buf.text += blk.text;
    } else if (blk.type === "thinking" && blk.thinking) {
      buf.thinking = (buf.thinking || "") + blk.thinking;
    } else if (blk.type === "tool_use") {
      buf.tools.set(blk.id, {
        id: blk.id, name: blk.name,
        input: typeof blk.input === "string" ? blk.input : JSON.stringify(blk.input ?? {}, null, 2),
        status: "running",
      });
    }
  }

  renderStream(mid);
  ensurePinned();
}

function renderStream(mid) {
  const buf = state.streamBuf.get(mid);
  if (!buf || !buf.el) return;
  const body = buf.el.querySelector(".body");
  body.innerHTML = "";

  if (buf.thinking) body.appendChild(cardEl("thinking", "💭", "Thinking", null, buf.thinking));
  if (buf.text) body.appendChild(mdEl(buf.text));
  for (const t of buf.tools.values()) {
    const sub = t.name === "Bash" ? firstLine(t.input) : "";
    body.appendChild(cardEl("tool", "✦", t.name, sub, t.input, t.status));
  }
}

function ingestResult(evt) {
  // a result frame can carry tool_result items in content; attach status to tools
  const content = evt.message && Array.isArray(evt.message.content) ? evt.message.content : [];
  for (const c of content) {
    if (c.type === "tool_result" && c.tool_use_id) {
      // find the owning stream's tool
      for (const buf of state.streamBuf.values()) {
        if (buf.tools && buf.tools.has(c.tool_use_id)) {
          const t = buf.tools.get(c.tool_use_id);
          t.status = c.is_error ? "error" : "done";
          t.output = typeof c.content === "string" ? c.content
                   : Array.isArray(c.content) ? c.content.map(x => x.text || "").join("\n") : "";
          // re-render that stream
          for (const [mid, b] of state.streamBuf) { if (b === buf) renderStream(mid); }
        }
      }
    }
  }
}

function finalizeStream() {
  // commit each finished stream's tools to error if still running (turn ended)
  for (const buf of state.streamBuf.values()) {
    if (buf.tools) for (const t of buf.tools.values()) if (t.status === "running") t.status = "done";
  }
  for (const mid of state.streamBuf.keys()) renderStream(mid);
  // Clear the assembly scratch: the rendered DOM stays on the page, but the
  // next turn must start with an empty buffer so the typing indicator shows
  // and a new message.id gets its own fresh shell.
  state.streamBuf.clear();
}

// =================== history backfill ===================
function ingestHistory(events) {
  // rebuild blocks from transcript events
  clearMessages();
  state.streamBuf.clear();
  for (const evt of events) {
    if (evt.type === "user" && evt.message) {
      const txt = contentText(evt.message.content);
      if (txt) echoUser(txt, evt.message.id);
    } else if (evt.type === "assistant" && evt.message) {
      // treat as a finalized stream
      ingestAssistant(evt);
    } else if (evt.type === "stderr") {
      pushStderr(str(evt.text));
    }
  }
  finalizeStream();
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
  body.textContent = bodyText || "";
  card.appendChild(head); card.appendChild(body);
  head.addEventListener("click", () => card.classList.toggle("open"));
  return card;
}

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
function nearBottom() {
  return scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 150;
}
function ensurePinned() {
  if (nearBottom()) scroller.scrollTop = scroller.scrollHeight;
  else $("newPill").classList.add("show");
}
scroller.addEventListener("scroll", () => {
  if (nearBottom()) $("newPill").classList.remove("show");
});
$("newPill").addEventListener("click", () => {
  scroller.scrollTop = scroller.scrollHeight;
  $("newPill").classList.remove("show");
});

// =================== busy / title ===================
function setBusy(b) {
  state.busy = b;
  sendBtn.classList.toggle("busy", b);
  sendBtn.textContent = b ? "■" : "↑";
  dot.classList.toggle("busy", b);
  if (b && !hasAssistantPending()) showPulse(true);
  else showPulse(false);
}
function hasAssistantPending() {
  for (const b of state.streamBuf.values()) if (b.el) return true;
  return false;
}
let pulseEl = null;
function showPulse(on) {
  if (on && !pulseEl) {
    pulseEl = document.createElement("div");
    pulseEl.className = "msg assistant";
    pulseEl.innerHTML = `<div class="body"><div class="pulse"><i></i><i></i><i></i></div></div>`;
    messagesEl.appendChild(pulseEl);
    emptyEl.classList.add("hidden");
    ensurePinned();
  } else if (!on && pulseEl) {
    pulseEl.remove(); pulseEl = null;
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
  state.streamBuf.clear();
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
