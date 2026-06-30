# Multi-device real-time session sync

## Goal

When a user stays on a session's chat page, that page must mirror in real time
across every paired device: messages and turn status ("LLM replying") produced
on one device (e.g. desktop) must appear on the others (e.g. mobile).

## Diagnosis (root cause, verified in code)

The server already broadcasts every turn event to all connected WebSocket
subscribers with `sessionId` attached (`manager.rs:96` `broadcast`, `:191-202`
runner fan-out) — streaming deltas, the final assistant frame, `turn_started` /
`turn_stopped` status, and `session_*` summaries. The transport for fan-out
exists. Three concrete gaps break the experience:

1. **User message never reaches other devices (primary).** In streaming mode the
   prompt is written to claude's stdin but never broadcast as an event
   (`claude.rs:244-256`); only the JSON-fallback path echoes a `user` frame
   (`:362-368`). The sender renders its own bubble optimistically
   (`lib.rs:312-327`, `app.js:716-723`). Other devices therefore see the answer
   with no question.

2. **Web/mobile client does not filter events by `sessionId`.**
   `app.js handleEvent` (`:127`) never checks `evt.sessionId` against the active
   session; desktop does (`lib.rs:1134`). On different sessions, activity bleeds
   into the wrong transcript.

3. **Opening a busy session shows no "replying" status.** Busy is driven only by
   the live `turn_started` event (`lib.rs:1174`, `app.js:133`). A device that
   opens/switches to a session whose turn is already running never sees
   `turn_started`, so no status shows.

## Scope

Steady-state sync (both devices on the page) plus busy-on-open. **Out of scope:**
replaying the partial reply already streamed before a device joins mid-turn (the
text appears from the next token / final frame onward; status still shows).

## Design

Reuse the existing broadcast + per-client event handlers. The user echo becomes a
single server-originated event; clients render it like any other broadcast frame,
which removes all client-side echo/dedup logic.

| Change | File | AC |
|---|---|---|
| `send()` broadcasts `{type:"user", sessionId, message:{role:"user", content:[{type:"text", text}]}}` before queueing the turn — the single source of the user echo | `server/manager.rs` | AC1 |
| Remove the now-redundant fallback user echo | `server/claude.rs` `:362-368` | AC1 |
| Drop optimistic local user echo; render the user bubble from the broadcast | `app/src/lib.rs` `on_sendClicked`; `app/web/app.js` `doSend` | AC1 |
| Drop the now-unneeded inbound user dedup | `lib.rs append_live_user`; `app.js echoUser` | AC1 |
| Filter transcript events by `sessionId` (mirror desktop); lifecycle + turn-status events bypass the filter | `app/web/app.js handleEvent` | AC4 |
| Track per-session running state from `turn_started`/`turn_stopped`/`bridge_error` for EVERY session (not just the open one), so the drawer dot and the cached state stay correct across devices | `app.js setSessionState`; `lib.rs set_session_state` + `dispatch_event` | AC5 |
| On session select, seed busy from the (now-fresh) cached `state == busy` | `lib.rs on_selectSession`; `app.js select` | AC5 |

### Ordering guarantee

`send()` broadcasts the user echo synchronously, before the runner picks up the
turn and emits `turn_started`. So every device receives:
`user` → `turn_started` → stream deltas → final `assistant` → `result` →
`turn_stopped`, in order.

### Tradeoff (accepted)

Dropping optimistic echo means the sender sees its own bubble after one server
round-trip (negligible on LAN; ~100ms on relay). This buys a dedup-free design.
Upgrade path if instant local echo is later wanted: client-generated `echoId`
passed in `op:send`, echoed back by the server, sender skips its own `echoId`.

### Status tracking (root cause of AC5)

Seeding busy from the cached state alone is insufficient: a turn started on
device A *after* B's last list/refresh leaves B's cached state stale-idle, so
opening the session shows no status. Root cause — the clients receive
`turn_started`/`turn_stopped` for **every** session (the server broadcasts to
all) but never used them to maintain per-session state. Fix: both clients update
their cached per-session state from those events for any session, ahead of the
active-session transcript filter. This makes the drawer's busy dot live AND makes
busy-on-open read a fresh state. No server change needed — the information was
already on the wire.

Residual ceiling: a turn that started while a device was fully disconnected is
reflected on reconnect via the `hello` snapshot (state read live server-side).

## Acceptance criteria (the contract)

Two devices paired to the same server, both able to view the same session.

1. **AC1** Both on session X. A sends a message → B shows A's user bubble (≤~1s).
   Both directions (desktop↔mobile).
2. **AC2** A's turn streams → B shows assistant text/thinking/tool cards
   streaming live. (Already works; verify no regression.)
3. **AC3** While A's turn runs, B shows "replying" status (typing pulse + busy
   dot + stop button); clears at turn end. Both directions.
4. **AC4** B is on a different session than A → A's activity does not appear in
   B's view.
5. **AC5** A turn is already running on X; B opens/switches to X → B immediately
   shows "replying" status; clears at turn end.

## Verification

- Server unit test: subscribe → create → send yields a `user` broadcast event
  before any turn output (covers AC1 server half), runnable without a live CLI.
- Manual against the live server: desktop + iOS simulator on the same session,
  exercise AC1–AC5. Restart block provided after server changes.
