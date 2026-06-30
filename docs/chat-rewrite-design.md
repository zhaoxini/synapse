# Chat Rewrite — Agent Chat Paradigm (ChatGPT/Claude mobile style)

## Diagnosis (root cause, verified)

The app is NOT a real agent chat because of **one protocol bug** in the client
renderer, plus a render-cost bug that makes it feel frozen.

### 1. The streaming protocol is never parsed (primary)
`claude -p --output-format stream-json --include-partial-messages` wraps the raw
Anthropic streaming events as:

```
{"type":"stream_event","event":{
    "type":"message_start", "message":{...}
}}
{"type":"stream_event","event":{
    "type":"content_block_start","index":0,
    "content_block":{"type":"thinking","thinking":""}   // or "text", or "tool_use"
}}
{"type":"stream_event","event":{
    "type":"content_block_delta","index":0,
    "delta":{"type":"thinking_delta","thinking":"The"}   // or "text_delta","input_json_delta"
}}
... (many deltas, per token) ...
{"type":"stream_event","event":{"type":"content_block_stop","index":0}}
{"type":"stream_event","event":{"type":"message_stop","stop_reason":"end_turn","usage":{...}}}
```

The current `ingest_event_into` (lib.rs:646) matches only top-level
`"assistant"` / `"user"` / `"result"`. **It never unwraps `stream_event`.**
Result: every partial token is dropped; the reply only appears when the final
complete `assistant` message arrives at end-of-turn. "Not streaming" = literally
never parsing the stream. Verified by capturing the real stream-json output and
counting: 11 `content_block_delta` events, all ignored by the client.

Also dropped: `thinking` blocks entirely (no reasoning display), and live
`tool_use` input assembly (`input_json_delta`).

The server side is correct — `claude.rs` forwards every event verbatim.

**Important reconciliation fact (verified):** a single real turn emits BOTH the
live `stream_event` deltas (e.g. 480 of them) AND the final complete frames —
5× top-level `assistant`, 1× `result`. So the client receives streaming deltas
*and* final assembled messages. The new parser must:
- Render live from `stream_event` deltas during the turn.
- When the final top-level `assistant`/`result` frame arrives, **reconcile by
  de-duplication**: if the streaming-built message for that turn already exists,
  replace its content with the authoritative final version (don't append a
  duplicate bubble). Key the turn's message by `message_id` (from
  `message_start`) so the final frame matches the live one.

### 2. Render cost bug (makes it feel frozen even for non-streamed parts)
`ingest_event()` does `get_messages().iter().collect::<Vec>()` then
`set_messages(new VecModel)` on **every event**. During a real turn that's many
times/second × full-list clone + full re-render. Even after we parse deltas, this
must become incremental (VecModel row updates) or streaming will stutter.

### 3. UI interaction gaps (separate, fixable)
- Keyboard occludes composer (iOS Slint/winit has no auto-inset).
- "Back" / drawer-return navigation rough.
- Tool blocks exist but the live-tool rendering is wrong because of #1.

## Data model

`MsgBlock` gains fields and a new variant for thinking. Each `MsgBlock` is one
**rendered row** (one bubble / one tool card / one thinking row / one code card).
A single assistant turn = several MsgBlocks (e.g. thinking row + text bubble +
tool card + text bubble).

New/changed `MsgBlock` fields:
- `kind`: existing — extend with `"thinking"`.
- `text`: existing.
- `messageId`: **new** — the Anthropic `message_id` this block belongs to. Drives
  de-dup reconciliation against the final top-level `assistant` frame.
- `blockIndex`: **new** — the `index` from `content_block_start`, scoped to its
  message, so deltas route to the right block during streaming.
- `toolName` / `toolStatus` / `toolId` / `expanded`: existing, kept.

**Streaming scratch state** (held on the UI thread for the active turn, reset on
`turn_stopped`):
- `current_message_id: Option<String>`
- `index_to_row: HashMap<usize, usize>` — block index → MsgBlock row in the model
  for the current message.

The live transcript stays a single `VecModel<MsgBlock>` (incremental updates via
`set_row_data`/`push`); no separate source-of-truth vec needed — the model IS the
source of truth, mutated in place.

## The fix — three layers

### Layer A: parse the stream protocol (the actual streaming)
Rewrite the ingest path to understand the streaming event sequence:

State machine over the stream, per turn:
- `message_start` → set `current_message_id`; remember it.
- `content_block_start{index, content_block}` → open block N of type
  `thinking` | `text` | `tool_use`. Push a placeholder MsgBlock; record
  `index → row`. Tool blocks get `toolName` from the block; default-collapsed.
- `content_block_delta{index, delta}` → route to block N by `index_to_row`, then
  `set_row_data` on just that row:
  - `text_delta.text` → append to text block
  - `thinking_delta.thinking` → append to thinking block
  - `input_json_delta.partial_json` → buffer onto the tool block; re-parse + re-render
    preview as it grows (gateway sends it whole here, but accumulate for safety)
- `content_block_stop{index}` → finalize: run the tool-input preview formatter,
  finalize any code-fence split for that text block.
- `message_delta` → capture `stop_reason`/`usage` if surfaced.
- `message_stop` → message done; clear `index_to_row` (next message starts fresh).
- Top-level `system` subtypes (`turn_started`/`turn_stopped` → busy/idle;
  `bridge_error` → error; `api_retry` → toast) — kept from current code.
- **Final-frame reconciliation**: top-level `assistant` frame → find the existing
  message row(s) by `messageId`; if present, replace with the authoritative final
  blocks (de-dup); else (non-streaming fallback) insert fresh. Top-level `result`
  → finalize turn, set idle.

This makes streaming, thinking, and tool-calls live, and prevents duplicate
bubbles from the final frames.

### Layer B: incremental rendering (smooth streaming)
Mutate the `VecModel` in place. On each delta, `set_row_data(row, …)` touches only
that row — Slint emits a row-granular change signal, so only the streaming bubble
re-renders, not the whole list. New blocks `push`. This replaces the per-event
`collect() → set_messages(new model)` full rebuild.

`normalize_code_blocks` (text↔code split) moves to **block-finalization** time
(`content_block_stop` / message end), not per-token — fences only re-split when a
block stops, not on every char. During streaming, a text block renders as plain
text; when it stops, if it contains fences it splits into text+code rows in place.

Throttle: coalesce incoming deltas behind a single `slint::Timer` fire (~33ms) —
buffer the latest state per row, flush once per frame. Prevents 480 deltas/turn
from scheduling 480 separate renders. (Cheap to add; keeps streaming smooth even
on the iOS sim which renders on the software path.)

### Layer C: UI interaction parity (ChatGPT/Claude mobile)
- **Keyboard inset**: decision — **add an explicit `keyboardInset` property** (set
  from Rust via UIKit `keyboardWillShow/Hide` notifications → safe-area inset).
  Reason: Slint's winit iOS backend resizes the *window* but does not inset the
  content, so the composer would sit under the keyboard without this. The chat
  container's bottom padding = `keyboardInset`; composer rides above it.
- **Composer**: floating pill, fixed above keyboard inset, send/stop toggle
  (already exists).
- **Tool blocks**: collapsible, default **collapsed** with a one-line summary
  (icon + tool name + status dot + short preview). Expand on tap. ChatGPT/Cursor/
  Claude-Code pattern: tools are noise by default, expandable on demand.
- **Thinking blocks**: collapsible "Thinking…" row, streaming text inside,
  collapsed by default (Claude/Cursor pattern).
- **Code blocks**: dark card, language label, copy button (already exists; keep).
- **Navigation**: drawer for sessions (exists); ensure tap-to-open + close (X,
  overlay tap, and selecting a session) all work — this is the "back" fix. Also
  confirm selecting a session closes the drawer and loads its history.

## Acceptance criteria (the contract)
Verified against the live server in the iOS simulator (and the desktop build, where
stderr is visible, for protocol debugging):

1. **Streaming**: during a turn, assistant text appears token-by-token in real
   time (not appearing all-at-once at turn end). Demonstrated with a multi-sentence
   reply.
2. **Thinking**: a turn that reasons shows a collapsible "Thinking" row streaming
   its reasoning; collapsed by default; expandable.
3. **Tool calls**: a turn that uses a tool shows a collapsible tool card appearing
   live (status running → done), with name + preview; default-collapsed; expandable
   to see full input/output. No duplicate bubbles from the final frame.
4. **No stutter**: a long reply (≥ several paragraphs, a real LS/Bash tool call)
   streams smoothly without frame drops or frozen UI.
5. **Keyboard**: tapping the composer opens the keyboard; the composer stays fully
   visible above it; dismissing the keyboard restores layout.
6. **Navigation**: ☰ opens the drawer; X / overlay-tap / selecting a session each
   close it and return to chat; the chat shows the selected session's history.
7. **Sessions are real**: the drawer shows the live Claude Code sessions (already
   true); selecting one loads its real transcript.

## Interaction reference (consensus across products)
- ChatGPT mobile: streaming text, collapsible reasoning, tool calls as compact
  chips expanded on tap, floating composer.
- Claude app: streaming, "Thinking" collapsible, artifacts/code rendered.
- Cursor: composer chat, tool steps collapsed.
- Claude Code CLI/TUI: streaming tokens, tool blocks as labeled collapsible
  groups with status.
Common spine: **stream text live; tools & thinking are collapsible noise by
default; composer floats above keyboard.**

## Build order (each step independently verifiable against the live server)
1. **Protocol + data model** (Layer A): unwrap `stream_event`, accumulate deltas,
   reconcile final frames. Verify criteria 1, 3 (de-dup). Debug on desktop (stderr
   visible) first, then sim.
2. **Incremental render + throttle** (Layer B): in-place VecModel, 33ms coalescing.
   Verify criterion 4.
3. **Thinking + tool blocks** (Layer C): collapsible rows, default-collapsed,
   live status. Verify criteria 2, 3.
4. **Keyboard inset** (Layer C). Verify criterion 5.
5. **Navigation polish** (Layer C). Verify criteria 6, 7.

Layers A & B are wired together (you can't stream without both), but C is
additive UI on top of the parsed blocks.
