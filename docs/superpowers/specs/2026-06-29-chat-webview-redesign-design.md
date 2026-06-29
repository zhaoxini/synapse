# Chat Redesign: Slint Pairing → WebView Chat

**Date:** 2026-06-29
**Status:** Approved (user: "你自己定就行，我只要结果")

## Problem

The chat page "整体都不好用". Root cause is not styling — it is that **Slint cannot render rich text or markdown**. Claude replies are markdown (headings, lists, **bold**, inline `code`, tables, multi-line code). The current UI renders every assistant message as a single `Text { wrap: word-wrap }`, producing a wall of text regardless of polish. Secondary pains all flow from fighting Slint's text model:

1. **No markdown** — unreadable assistant text.
2. **Auto-scroll fights the user** — `viewport-y` is hard-bound to bottom; reading history mid-response is impossible.
3. **Code blocks blow out the page** — no max height / internal scroll; a 200-line file makes the whole conversation unscrollable.
4. **Streaming feels detached** — pulse-dots row separate from the bubble; tokens don't appear inline.
5. **Tool cards dump raw text** — Bash output, file reads not usefully summarized.

## Decision

**Move the entire post-pairing experience to a webview.** The chat surface, composer, drawer, session list, and top bar become one coherent HTML/CSS/JS app. Slint is retained **only** for the pairing screen (it works; leave it).

**Why webview over alternatives:**
- *Slint + Rust markdown parser:* medium effort, medium ceiling — tables, nested formatting, syntax colors all still limited. A patch, not a fix.
- *Native SwiftUI rewrite:* highest effort, throws away Slint + iOS shim + desktop target.
- *WebView:* highest quality ceiling (real markdown, syntax highlight, native momentum scroll, trivial copy/collapse/diffs), server bridge untouched, matches Synara/ChatGPT feel. **Chosen.**

## Architecture

```
Slint pairing screen  ──connect──►  Rust resolves {url, token}
                                        │
                                        ▼
                              hide Slint, show WKWebView
                                        │
              [chat web app: HTML+CSS+JS, bundled offline]
                                        │
                          ◄── WebSocket ──►  Rust server (UNCHANGED)
```

**One clean handoff, no per-message bridge.** After pairing, Rust injects
`window.__SYNAPSE__ = { url, token }` into the webview. The web app is a
normal WS client speaking the existing protocol (`{op:"send"...}` / event
stream). There is no Rust↔webview message pump: JS owns connection lifecycle,
reconnect, and all rendering. The Slint↔webview boundary is a single transition
(pair → inject credentials → show webview).

This means the WS protocol handling currently in `net.rs` + `handle_event`
is **replicated in JS**, not bridged. That is intentional — owning the whole
post-pairing experience is what makes it feel like an app instead of a render
target.

## The web chat app

| Area | Behavior |
|---|---|
| **Assistant text** | Real markdown via `marked.js` — headings, lists, bold, inline code, tables, blockquotes. |
| **Code blocks** | `highlight.js` syntax color · language label + **Copy** (flash ✓) · **max-height + internal scroll** so long files can't blow up the page. |
| **Scroll** | Auto-pins to bottom **only when already near bottom** (~150px). Scrolled up → never yanks back. A **"↓ new"** pill appears when content arrives off-screen. |
| **Streaming** | Tokens append **live into the bubble**, markdown re-renders in place. Pulse-dots only *before* the first token. |
| **Composer** | Multiline, grows with content. `↑` send → `■` stop when busy. Rides the iOS keyboard. |
| **Tool / thinking** | Collapsible cards, summarize by default (tool name + status), expand for raw output. |
| **Drawer / sessions** | Slide-over panel, search, active highlight, new session — rebuilt in web so the whole UI is consistent. |
| **Gestures** | Native momentum scroll, tap-code-to-copy, long-press message → copy. |

## Protocol contract (JS must replicate)

From `net.rs` / `http.rs`:

- **Connect:** `ws(s)://host:port/?token=T` (direct) or `ws(s)://host:port/connect?token=T` (relay). Self-signed TLS accepted (webview WKWebView handles via ATS exception / permissive cert; same trust posture as current `AcceptAnyCert`).
- **Outbound ops:** `{"op":"list"}`, `{"op":"create","opts":{cwd,name,model,permission_mode,agent}}`, `{"op":"send","sessionId","content"}`, `{"op":"stop","sessionId"}`, `{"op":"refresh"}`, `{"op":"history","sessionId","limit"}`.
- **Inbound frames:** `{"type":"hello","sessions":[...]}`, `{"type":"event","event":{...}}` (event types: `system` (subtypes: `session_created`, `turn_started`, `turn_stopped`, `bridge_error`, `fallback_to_json`), `assistant`, `user`, `stderr`, `result`), `{"type":"created","session":{...}}`, `{"type":"sessions","sessions":[...]}`, `{"type":"history","sessionId","events","found"}`, `{"type":"error",...}`.
- **Reconnect:** capped exponential backoff (1s → 15s), same as `run_connection`. On reconnect, re-request active session transcript (`op:history`).

## Assistant frame assembly (JS replicates `assemble_assistant_blocks`)

The server streams `assistant` frames where `message.content[]` holds blocks:
`text`, `tool_use`, and `thinking`. JS assembles per `message.id`:
- `text` → markdown bubble (streamed, re-render on each chunk).
- `tool_use` → collapsible tool card (name from `name`, status from a later `tool_result` or progress).
- `thinking` → collapsible thinking card.
- Auth/quota error frames have no `message.id` but carry `message.content[0].text` with the error → render as an error card (mirrors the `lib.rs` `mid.is_empty()` fix).

## Layout of new code

```
crates/app/
  web/                      # the chat web app (bundled offline)
    index.html
    app.css
    app.js                  # WS client + rendering
    vendor/
      marked.min.js
      highlight.min.js
      github-dark.min.css   # highlight theme
  build.rs                  # embed web/ via include_dir or include_str! macro
```

Web assets are embedded into the binary at build time (offline, no network dep).
Rust serves them to the webview via `loadFileURL` / `loadHTMLString` with a base
URL so relative asset paths resolve.

## iOS hosting

After Slint pairing completes (existing flow calls `set_view("chat")`), instead
of showing the Slint chat view:

1. Rust builds the WS URL from the parsed pairing link.
2. Hide the Slint surface; instantiate a `WKWebView` over the app window.
3. Load the bundled `index.html`; inject `window.__SYNAPSE__ = { url, token }` before the chat app's `connect()`.
4. The web app connects and owns everything from there.

The iOS Slint window and the webview coexist; webview is shown when in chat,
Slint remains underneath for the pairing screen (and as fallback). Detail of
whether to keep Slint's window alive or replace it is an implementation detail
resolved in build — the contract is "pairing in Slint, chat in webview".

## Skipped (add when asked)

- In-web pairing (keeps working Slint pairing screen).
- Diff/PR UI (Synara-specific desktop feature; out of scope for mobile remote control).
- Message edit-resend, session branching.
- Android webview integration (iOS first; same pattern applies later).

## Scope check

Single implementation plan. Three build units: (1) web bundle, (2) iOS webview
hosting + credential injection, (3) Rust build-time embed of web assets. Server
is unchanged. Desktop app keeps building (chat UI served from same embedded
bundle in a desktop webview, or via the existing Slint chat as fallback —
resolved in build).
