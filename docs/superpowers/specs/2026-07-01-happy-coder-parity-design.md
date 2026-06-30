# Synapse → Happy Coder parity — design / acceptance contract

Date: 2026-07-01. Goal: complete the remaining *core* features of Synapse so it
reaches feature parity with Happy Coder for the things that matter to a mobile
remote-control of the Claude Code CLI, in Synara's interaction/visual style.

## Ground truth (verified this session)

- Shipped chat surface = `crates/app/web/` (HTML/CSS/JS bundle, embedded in the
  iOS WKWebView and the desktop `synapse-web` host). The Slint chat view and the
  `web/` Node prototype are NOT the shipped surface. **All client work lands in
  `crates/app/web/`.**
- Server = `crates/server`. Each turn spawns `claude -p --input-format
  stream-json --output-format stream-json --include-partial-messages [--resume]`,
  writes ONE user message to stdin, then **drops stdin**. Events are forwarded
  verbatim to every WS client. Session state is in-memory only; transcripts live
  in `~/.claude/projects/<enc-cwd>/<sid>.jsonl` (history backfill + live tail).
- `claude` is v2.1.196, a compiled Bun binary. It has NO `--permission-prompt-tool`
  flag. Probe result: with plain stream-json + `--permission-mode default`, a
  permission-gated Bash call was auto-run (repo allowlist) and **no
  `control_request`/`can_use_tool` was emitted**. Therefore interactive approve
  requires the wrapper to speak the **SDK control protocol**: an `initialize`
  handshake that registers a `can_use_tool` callback, then a `control_response`
  per request. The wire symbols (`canUseTool`, `can_use_tool`,
  `control_request`/`control_response`, `permission_denials`, `updatedInput`,
  `behavior:allow|deny`) are confirmed present in installed SDKs; exact shapes to
  be extracted from a readable SDK before Phase 2 coding.

## Build order & acceptance criteria (the contract)

### Phase 1 — Rich chat rendering (client-only, `crates/app/web/`)
Renders the *different data structures* Claude Code emits as first-class views
instead of a generic JSON card. No server/protocol changes.

- **1a Diff viewer.** Edit/MultiEdit render a unified line diff of
  `old_string`→`new_string` (LCS-based, handwritten — no dep); Write renders its
  content as all-added; NotebookEdit renders cell source. Header = basename of
  `file_path`; stat = `+adds −dels`; +/- gutter using the theme tokens
  `--color-editor-added/-deleted` + `--color-decoration-added/-deleted`. Long
  diffs fold like code blocks.
  - AC1: an Edit tool call shows a red/green line diff with a `+N −M` stat and the
    file name; verified in the iOS sim against a real edit turn.
- **1b Per-tool views.** Bash (command line + stdout/stderr, terminal styling),
  Read/LS/Glob/Grep (path/pattern + result), TodoWrite (checklist with state),
  WebFetch/WebSearch (url/host/query), MCP/other (current generic card). Tool
  title + one-line subtitle derived per tool.
  - AC2: a Bash turn shows the command as the card subtitle and stdout in the body;
    a TodoWrite turn shows a checklist.
- **1c AskUserQuestion (read-only) + ExitPlanMode plan (read-only).** Render the
  question + options, and the plan markdown, as distinct cards. Interactive
  answering/approval is Phase 2 (needs the response channel).
  - AC3: an AskUserQuestion tool call shows the question and its options; an
    ExitPlanMode call shows the plan as formatted markdown.

### Phase 2 — Permission / approve
- **2a Permission-mode switcher.** Make `permission_mode` mutable per session
  (mirror `set_model`): new WS op `set_permission_mode`; composer pill cycles
  default / acceptEdits / plan / bypassPermissions; persisted on the session
  summary and applied to the next turn's `claude -p` args.
  - AC4: switching a session to `acceptEdits` causes the next edit turn to apply
    without a denial; the pill reflects the active mode across devices.
- **2b Per-request approve/deny over the control protocol.** Server keeps stdin
  open for the turn, performs the `initialize` handshake declaring `canUseTool`,
  parses `can_use_tool` control_requests, emits a `permission_request` WS event
  `{requestId, sessionId, tool, input, suggestions}`; client renders an approve
  footer under the pending tool (with the diff for edits) offering: Allow once /
  Allow for session / Allow-all-edits (→acceptEdits) / Deny; client replies
  `permission_response {requestId, behavior, updatedInput?, mode?}`; server writes
  the `control_response`.
  - AC5: in `default` mode, a non-allowlisted Bash call surfaces an approve/deny
    footer on the phone; Allow runs it, Deny returns the denial to the model;
    verified in the iOS sim.

### Phase 3 — Session lifecycle
- WS ops `delete`, `archive`, `rename`, `kill` (kill the whole session, distinct
  from `stop` which interrupts a turn). Drawer row swipe/long-press actions.
  Archived sessions hidden behind a toggle. Rename sets a sticky title overriding
  the first-user-line heuristic.
  - AC6: a session can be renamed, archived (hidden), and deleted from the drawer;
    a running session can be killed.

### Phase 4 — Notifications + settings polish
- Local notification (Web Notifications API in the WebView where available; iOS
  native hook later) on turn-finished and permission-requested while the app is
  backgrounded. A small settings sheet (mode default, notifications on/off,
  connection info, disconnect/re-pair).
  - AC7: finishing a turn while backgrounded posts a notification (where the
    platform allows); settings sheet can disconnect.

## Out of scope (explicit YAGNI for a single-user mobile remote)
Voice, friends/social, artifacts/notes, file browser/editor, usage/cost
dashboards, command palette, multi-machine/device management beyond the existing
relay+tunnel pairing. Revisit only if the `/loop` self-check proves one is core.

## Verification discipline
Every phase is verified on the **real shipped surface** (rebuild the embedded
bundle, run the iOS sim app), not just a browser tab — the bundle is compiled
into the binary. Server changes ship with a paste-ready rebuild+restart+health
block. `cargo fmt`/`clippy -D warnings`/`cargo test` stay green.

## Self-check — Happy core-feature coverage (2026-07-01)

Bucket-by-bucket review of Happy's core inventory vs Synapse after this work.

1. **Session management — DONE.** list, create (remote), resume/attach, switch,
   search, model picker, project picker, auto-title, **rename**, **delete/hide**,
   kill-via-stop, multi-session. Descoped: spawn-on-specific-machine (one
   server = one machine), reversible archive (delete covers hide), unread dots.
2. **Chat / approve — DONE.** streaming, thinking, tool_use/tool_result (+live
   routing fix), errors, turn lifecycle, **per-request approve/deny**
   (e2e-verified), **permission-mode switcher**, per-tool views (Edit/Bash/Read/
   Grep/Glob/LS/TodoWrite/WebFetch/WebSearch), AskUserQuestion + ExitPlanMode
   (render), markdown/code/fold/copy. Nice-to-have gaps: slash-command chip/
   autocomplete, file/attachment composer, interactive AskUserQuestion answering,
   nested subagent/Task streaming.
3. **Diff viewing — DONE.** unified line diff for Edit/MultiEdit/Write/
   NotebookEdit, fold, `+/−` stat, also shown inside the approve card. Gaps:
   split view, git-status surfaces (nice-to-have).
4. **Device / machine — pairing & remote-access DONE** (QR, token, TLS,
   Cloudflare tunnel, self-hosted relay). Multi-machine-in-one-app DESCOPED
   (architectural; low single-user value — the relay already reaches the machine).
5. **Notifications — NOT DONE (deferred).** Core-ish for a remote tool; needs
   push infra (APNs) or Web Notifications (limited in WKWebView). Next candidate.
6. **Voice / friends / artifacts / file-browser / usage / command-palette —
   DESCOPED** (not core for a single-user mobile remote).
7. **Connectivity — DONE** (WS, reconnect, multi-device broadcast/echo, relay).
   Zero-knowledge E2E encryption DESCOPED (Synapse uses TLS + pairing token, a
   different trust model).
8. **Auth / onboarding — DONE** (QR pairing, token, multi-device via relay).
   Accounts / GitHub DESCOPED.

**Conclusion:** every CORE bucket the user named (session mgmt, chat/approve,
diff, device) is covered or deliberately descoped with rationale. The one
core-ish deferral is notifications (needs platform infra).

**Verification status:** approve flow **e2e-verified through synapse-server**
(create → permission_request → allow → tool executes); 15 server tests +
`lineDiff` self-check green; full workspace builds; JS syntax checked. PENDING:
on-device visual (iOS sim / live browser) — blocked this session by the shared
browser being held by a concurrent process.
