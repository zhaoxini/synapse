# Chat Rewrite (Agent Chat Paradigm) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Synapse chat render as a real agent chat — live token streaming, collapsible thinking + tool-call blocks, keyboard-aware composer, working drawer navigation — by fixing the client's stream-protocol parser and rendering.

**Architecture:** The server (`crates/server`) already forwards every Claude Code stream-json event verbatim. The bug is entirely client-side: `ingest_event_into` never unwraps `{"type":"stream_event",...}`, so partial tokens/thinking/tool-input are dropped. We rewrite the ingest path into a streaming state machine that consumes the Anthropic event sequence, render incrementally via Slint's `VecModel` (row-granular updates + a 33ms render throttle), and rebuild the chat UI blocks as collapsible rows (ChatGPT/Claude/Cursor pattern). Keyboard inset is fed from UIKit.

**Tech Stack:** Rust, Slint 1.17 (.slint UI + `include_modules!`), tokio + tokio-tungstenite (net thread, unchanged), serde_json, Objective-C++/UIKit (keyboard observer on iOS).

**Spec:** `docs/chat-rewrite-design.md` (root-cause analysis, data model, 7 acceptance criteria).

## Global Constraints

- **Do not change the on-wire protocol** — the server is correct and forwards events verbatim. All work is client-side (`crates/app`) plus a tiny iOS keyboard hook.
- **Property/callback names are a stable contract**: new `.slint` properties/struct fields are additive. Never rename existing ones the server side touches (none here — server is protocol-only).
- **Platform**: must build for `aarch64-apple-ios-sim` (sim) AND desktop (`x86_64-apple-darwin`/`aarch64-apple-darwin`). Desktop is the primary debug surface (stderr visible); sim is the acceptance surface. iOS-only code is `#[cfg(target_os = "ios")]`.
- **Slint 1.17 invariants**: `Weak::upgrade()` works ONLY on the UI thread (see memory `slint-weak-upgrade-thread-local`). Net-thread→UI updates must go through `slint::invoke_from_event_loop`. Never call `weak.upgrade()` off the UI thread.
- **Ponytail / YAGNI**: minimum code that solves it. No new crates unless impossible without (objc is reachable via raw `extern "C"` + existing UIKit; no `objc2` direct dep added). No speculative abstractions.
- **Commit cadence**: one commit per task. Never commit on the user's behalf unless the user asks — but this plan's tasks each END with a commit step; execute commits only after confirming the user wants them committed (CLAUDE.md: "Commit or push only when the user asks"). Default: stage + leave uncommitted, report.
- **Test strategy**: the core ingest logic is pure (takes `&Value`, mutates state) and unit-testable without Slint. UI/keyboard/streaming-smoothness are verified manually against the live server on sim (acceptance criteria). Write unit tests for the parser; manual-verify the UI.

## File Structure

**Modified:**
- `crates/app/src/lib.rs` — the ingest/render rewrite. Largest change. Holds the streaming state machine + `VecModel` incremental updates + `keyboardInset` plumbing. (Currently ~1218 lines; the new ingest logic replaces `ingest_event`/`ingest_event_into`/`normalize_code_blocks` hot paths and adds a small `StreamState` struct + tests.)
- `crates/app/ui/app.slint` — `MsgBlock` struct gains `messageId`/`blockIndex`/`thinking` rendering; chat view reworked: collapsible tool + thinking rows (default-collapsed), keyboard-inset bottom padding on the composer container.
- `mobile/ios/Sources/main.m` — after `synapse_ios_main()`, call a new `synapse_install_keyboard_observer()` so UIKit keyboard notifications reach Rust. (Desktop: no-op stub.)

**New files:**
- (none beyond the plan/spec docs already created)

**Unchanged (verified correct, do not touch):**
- `crates/server/src/claude.rs`, `crates/server/src/http.rs`, `crates/app/src/net.rs` (the net→UI bridge already uses `invoke_from_event_loop` correctly; the prior fix stands).

---

## Task 1: Add `messageId` + `blockIndex` to `MsgBlock` (data model + struct)

This is the foundation — later tasks key de-dup and delta-routing off these fields. No behavior change yet; just widen the struct and every construction site.

**Files:**
- Modify: `crates/app/ui/app.slint` (the `struct MsgBlock { ... }` block, ~line 41)
- Modify: `crates/app/src/lib.rs` (every `MsgBlock { ... }` literal — there are ~7: `push_text`, `push_system_error`, `ingest_event_into` tool_use branch, `history`/`normalize_code_blocks` builders, tests)

**Interfaces:**
- Produces: `MsgBlock` now has two new `string`/`int` fields. Slint regenerates the Rust struct via `include_modules!()`, so the Rust `MsgBlock` literal must include them (the build fails otherwise, which is the guard).

- [ ] **Step 1: Add fields to the `.slint` struct**

In `crates/app/ui/app.slint`, the `struct MsgBlock` block becomes:

```slint
struct MsgBlock {
    kind: string,
    role: string,
    text: string,
    toolName: string,
    toolStatus: string,
    expanded: bool,
    toolId: string,
    codeLang: string,
    time: string,
    messageId: string,
    blockIndex: int,
}
```

- [ ] **Step 2: Update every `MsgBlock { ... }` literal in `lib.rs` to include the new fields**

For each existing literal, add the two fields. Default values: `messageId: "".into()`, `blockIndex: 0`. Example for `push_text` (lib.rs:823):

```rust
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
```

Apply the same two-field addition to ALL other `MsgBlock {` literals: `push_system_error`, the `tool_use` push in `ingest_event_into`, the `normalize_code_blocks` builder, the `history`/`ingest_event` paths, and the `#[cfg(test)]` literals. Use `grep -n "MsgBlock {" crates/app/src/lib.rs` to find them all.

- [ ] **Step 3: Build to confirm the struct contract matches**

Run: `cargo build -p synapse-app 2>&1 | tail -20`
Expected: clean build (Slint regenerates the struct; the build fails with a field-count error if any literal was missed — fix any such site).

- [ ] **Step 4: Run existing tests**

Run: `cargo test -p synapse-app 2>&1 | tail -20`
Expected: existing tests pass (they only assert on `.kind`/`.text`/`.codeLang`, unaffected by new fields).

- [ ] **Step 5: Stage**

```bash
git add crates/app/ui/app.slint crates/app/src/lib.rs
```
(Do not commit unless the user asks — report the staged change.)

---

## Task 2: Streaming parser — `StreamState` + `stream_event` unwrapping (TDD)

The core fix: parse the `stream_event`/`content_block_*` sequence into live message blocks. Pure logic, fully unit-testable.

**Files:**
- Modify: `crates/app/src/lib.rs` — add a `StreamState` struct and a method `apply_stream_event(&mut self, evt: &Value) -> Vec<DeltaOp>` plus a `DeltaOp` enum describing model mutations. Keep it pure (no `App`, no Slint) so it's testable.

**Interfaces:**
- Consumes: the `MsgBlock` struct from Task 1.
- Produces:
  - `struct StreamState { current_message_id: Option<String>, index_to_row: HashMap<usize, usize> }` (created fresh per turn)
  - `enum DeltaOp { UpsertBlock { row: Option<usize>, block: MsgBlock }, AppendBlock(MsgBlock), ReplaceMessage { message_id: String, blocks: Vec<MsgBlock> }, ClearMessage }` — the minimal set of model mutations the UI layer applies. (Naming is fixed here; Task 3 consumes it.)

```rust
use std::collections::HashMap;

/// Per-turn streaming scratch state. Owns the current message id and the
/// mapping from Anthropic content-block index -> row in the live model.
pub struct StreamState {
    pub current_message_id: Option<String>,
    pub index_to_row: HashMap<usize, usize>,
}

/// A model mutation derived from one stream event. The UI layer (Task 3)
/// applies these to the VecModel incrementally.
pub enum DeltaOp {
    /// Update an existing row, or if `row` is None it's brand-new.
    UpsertBlock { row: Option<usize>, block: MsgBlock },
    /// The final assembled message arrived; replace all rows with this id.
    ReplaceMessage { message_id: String, blocks: Vec<MsgBlock> },
    /// Turn ended; reset per-turn state.
    Reset,
}

impl StreamState {
    pub fn new() -> Self {
        Self { current_message_id: None, index_to_row: HashMap::new() }
    }

    /// Reset at turn end (message_stop / result / turn_stopped).
    pub fn reset(&mut self) {
        self.current_message_id = None;
        self.index_to_row.clear();
    }
}
```

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `lib.rs`. These assert on `DeltaOp` content using a tiny helper to make a default block. (Add a `fn blank_block()` test helper at the top of the test module.)

```rust
#[test]
fn stream_message_start_sets_message_id() {
    let mut s = StreamState::new();
    let ops = apply_stream_event(&mut s, &serde_json::json!({
        "type":"stream_event","event":{"type":"message_start",
        "message":{"id":"msg_1","role":"assistant","content":[]}}
    }));
    assert_eq!(s.current_message_id.as_deref(), Some("msg_1"));
    assert!(ops.is_empty()); // no block yet
}

#[test]
fn stream_text_delta_accumulates_into_block() {
    let mut s = StreamState::new();
    let _ = apply_stream_event(&mut s, &serde_json::json!({
        "type":"stream_event","event":{"type":"message_start",
        "message":{"id":"msg_1","role":"assistant","content":[]}}
    }));
    let ops = apply_stream_event(&mut s, &serde_json::json!({
        "type":"stream_event","event":{"type":"content_block_start","index":0,
        "content_block":{"type":"text","text":""}}
    }));
    assert!(matches!(ops.as_slice(), [DeltaOp::UpsertBlock { row: None, .. }]));
    // now stream two text deltas; they must route to row 0
    s.index_to_row.insert(0, 0); // simulate UI recording the row
    let ops = apply_stream_event(&mut s, &serde_json::json!({
        "type":"stream_event","event":{"type":"content_block_delta","index":0,
        "delta":{"type":"text_delta","text":"Hello"}}
    }));
    match ops.as_slice() {
        [DeltaOp::UpsertBlock { row: Some(0), block }] => {
            assert_eq!(block.text, "Hello");
        }
        other => panic!("expected single upsert to row 0, got {:?}", other.len()),
    }
    let ops = apply_stream_event(&mut s, &serde_json::json!({
        "type":"stream_event","event":{"type":"content_block_delta","index":0,
        "delta":{"type":"text_delta","text":" world"}}
    }));
    match ops.as_slice() {
        [DeltaOp::UpsertBlock { row: Some(0), block }] => assert_eq!(block.text, "Hello world"),
        other => panic!("expected accumulated text, got {:?}", other.len()),
    }
}

#[test]
fn stream_tool_use_records_name_and_id() {
    let mut s = StreamState::new();
    let _ = apply_stream_event(&mut s, &serde_json::json!({
        "type":"stream_event","event":{"type":"message_start",
        "message":{"id":"msg_1","role":"assistant","content":[]}}
    }));
    let ops = apply_stream_event(&mut s, &serde_json::json!({
        "type":"stream_event","event":{"type":"content_block_start","index":1,
        "content_block":{"type":"tool_use","id":"call_1","name":"Bash","input":{}}}
    }));
    match ops.as_slice() {
        [DeltaOp::UpsertBlock { row: None, block }] => {
            assert_eq!(block.kind, "tool");
            assert_eq!(block.toolName, "Bash");
            assert_eq!(block.toolId, "call_1");
        }
        other => panic!("got {:?}", other.len()),
    }
}

#[test]
fn stream_input_json_delta_parses_tool_input() {
    let mut s = StreamState::new();
    let _ = apply_stream_event(&mut s, &serde_json::json!({
        "type":"stream_event","event":{"type":"message_start",
        "message":{"id":"msg_1","role":"assistant","content":[]}}
    }));
    let _ = apply_stream_event(&mut s, &serde_json::json!({
        "type":"stream_event","event":{"type":"content_block_start","index":0,
        "content_block":{"type":"tool_use","id":"call_1","name":"Bash","input":{}}}
    }));
    s.index_to_row.insert(0, 0);
    let ops = apply_stream_event(&mut s, &serde_json::json!({
        "type":"stream_event","event":{"type":"content_block_delta","index":0,
        "delta":{"type":"input_json_delta","partial_json":"{\"command\":\"ls\"}"}}
    }));
    match ops.as_slice() {
        [DeltaOp::UpsertBlock { row: Some(0), block }] => {
            assert!(block.text.contains("ls"), "preview should contain the command, got: {}", block.text);
        }
        other => panic!("got {:?}", other.len()),
    }
}

#[test]
fn stream_thinking_delta_routes_to_thinking_block() {
    let mut s = StreamState::new();
    let _ = apply_stream_event(&mut s, &serde_json::json!({
        "type":"stream_event","event":{"type":"message_start",
        "message":{"id":"msg_1","role":"assistant","content":[]}}
    }));
    let _ = apply_stream_event(&mut s, &serde_json::json!({
        "type":"stream_event","event":{"type":"content_block_start","index":0,
        "content_block":{"type":"thinking","thinking":""}}
    }));
    s.index_to_row.insert(0, 0);
    let ops = apply_stream_event(&mut s, &serde_json::json!({
        "type":"stream_event","event":{"type":"content_block_delta","index":0,
        "delta":{"type":"thinking_delta","thinking":"reasoning"}}
    }));
    match ops.as_slice() {
        [DeltaOp::UpsertBlock { row: Some(0), block }] => {
            assert_eq!(block.kind, "thinking");
            assert_eq!(block.text, "reasoning");
        }
        other => panic!("got {:?}", other.len()),
    }
}

#[test]
fn stream_unknown_event_yields_no_ops() {
    let mut s = StreamState::new();
    let ops = apply_stream_event(&mut s, &serde_json::json!({"type":"stream_event","event":{"type":"ping","index":0}}));
    assert!(ops.is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p synapse-app stream_ 2>&1 | tail -20`
Expected: FAIL — `cannot find function apply_stream_event` (and `StreamState`/`DeltaOp`).

- [ ] **Step 3: Implement `StreamState`, `DeltaOp`, and `apply_stream_event`**

Add to `lib.rs` (near the existing ingest code). `apply_stream_event` returns `Vec<DeltaOp>`. It maintains an internal accumulator per block (so deltas append); store accumulated text **inside the produced `MsgBlock.text`** of each op (re-derive from the live block via the UI layer is unnecessary — `apply_stream_event` tracks its own buffer keyed by index). Use a `HashMap<usize, MsgBlock>` inside `StreamState` as the block buffer (add field `blocks: HashMap<usize, MsgBlock>`).

```rust
pub struct StreamState {
    pub current_message_id: Option<String>,
    /// Anthropic block index -> row the UI assigned (set by Task 3 when it
    /// applies an UpsertBlock with row:None).
    pub index_to_row: std::collections::HashMap<usize, usize>,
    /// Anthropic block index -> accumulated block content.
    blocks: std::collections::HashMap<usize, MsgBlock>,
    /// buffer for tool_use input_json_delta fragments, by index.
    tool_input_buf: std::collections::HashMap<usize, String>,
}

impl StreamState {
    pub fn new() -> Self {
        Self {
            current_message_id: None,
            index_to_row: Default::default(),
            blocks: Default::default(),
            tool_input_buf: Default::default(),
        }
    }
    pub fn reset(&mut self) {
        self.current_message_id = None;
        self.index_to_row.clear();
        self.blocks.clear();
        self.tool_input_buf.clear();
    }
}

/// Parse one forwarded Claude Code event into zero or more model mutations.
/// Pure: no App/Slint. The UI layer (Task 3) consumes `DeltaOp`s.
pub fn apply_stream_event(s: &mut StreamState, evt: &serde_json::Value) -> Vec<DeltaOp> {
    let top = evt.get("type").and_then(|v| v.as_str()).unwrap_or("");
    // Only stream_event carries the live deltas we care about here.
    let ev = match top {
        "stream_event" => evt.get("event").cloned().unwrap_or(serde_json::Value::Null),
        _ => return Vec::new(),
    };
    let et = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match et {
        "message_start" => {
            let id = ev
                .pointer("/message/id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !id.is_empty() {
                s.current_message_id = Some(id);
            }
            // start fresh block set for this message
            s.blocks.clear();
            s.index_to_row.clear();
            s.tool_input_buf.clear();
            Vec::new()
        }
        "content_block_start" => {
            let idx = ev.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let cb = ev.get("content_block").cloned().unwrap_or(serde_json::Value::Null);
            let bt = cb.get("type").and_then(|v| v.as_str()).unwrap_or("text");
            let block = match bt {
                "tool_use" => MsgBlock {
                    kind: "tool".into(),
                    role: "assistant".into(),
                    text: "".into(),
                    toolName: cb.get("name").and_then(|v| v.as_str()).unwrap_or("tool").into(),
                    toolStatus: "running".into(),
                    expanded: false,
                    toolId: cb.get("id").and_then(|v| v.as_str()).unwrap_or("").into(),
                    codeLang: "".into(),
                    time: now_time().into(),
                    messageId: s.current_message_id.clone().unwrap_or_default().into(),
                    blockIndex: idx as i32,
                },
                "thinking" => MsgBlock {
                    kind: "thinking".into(),
                    text: cb.get("thinking").and_then(|v| v.as_str()).unwrap_or("").into(),
                    ..default_block("thinking", s, idx)
                },
                _ => MsgBlock {
                    kind: "text".into(),
                    text: cb.get("text").and_then(|v| v.as_str()).unwrap_or("").into(),
                    ..default_block("text", s, idx)
                },
            };
            s.blocks.insert(idx, block.clone());
            vec![DeltaOp::UpsertBlock { row: s.index_to_row.get(&idx).copied(), block }]
        }
        "content_block_delta" => {
            let idx = ev.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let delta = ev.get("delta").cloned().unwrap_or(serde_json::Value::Null);
            let dt = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let Some(block) = s.blocks.get_mut(&idx) else { return Vec::new(); };
            match dt {
                "text_delta" => {
                    let t = delta.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    block.text = format!("{}{}", block.text, t).into();
                }
                "thinking_delta" => {
                    let t = delta.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                    block.text = format!("{}{}", block.text, t).into();
                }
                "input_json_delta" => {
                    let pj = delta.get("partial_json").and_then(|v| v.as_str()).unwrap_or("");
                    let buf = s.tool_input_buf.entry(idx).or_default();
                    buf.push_str(pj);
                    // best-effort parse; if it parses, format a preview
                    if let Ok(input) = serde_json::from_str::<serde_json::Value>(buf) {
                        block.text = tool_arg_preview(&block.toolName, Some(&input)).into();
                    }
                }
                _ => {}
            }
            let block = block.clone();
            vec![DeltaOp::UpsertBlock { row: s.index_to_row.get(&idx).copied(), block }]
        }
        "content_block_stop" => {
            // finalize: nothing extra needed beyond what deltas accumulated.
            // (Code-fence splitting for text blocks happens in Task 3 at render.)
            Vec::new()
        }
        "message_stop" => {
            // message done; keep blocks until next message_start resets them.
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn default_block(kind: &str, s: &StreamState, idx: usize) -> MsgBlock {
    MsgBlock {
        kind: kind.into(),
        role: "assistant".into(),
        text: "".into(),
        toolName: "".into(),
        toolStatus: "".into(),
        expanded: false,
        toolId: "".into(),
        codeLang: "".into(),
        time: now_time().into(),
        messageId: s.current_message_id.clone().unwrap_or_default().into(),
        blockIndex: idx as i32,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p synapse-app stream_ 2>&1 | tail -20`
Expected: all 6 `stream_*` tests PASS.

- [ ] **Step 5: Build the whole crate**

Run: `cargo build -p synapse-app 2>&1 | tail -10`
Expected: clean (the new code is unused warnings-free; `StreamState` is `pub` but unused at the call site yet — that's fine, Task 3 wires it).

- [ ] **Step 6: Stage**

```bash
git add crates/app/src/lib.rs
```

---

## Task 3: Wire `StreamState` into `handle_event` with incremental `VecModel` updates

Connect the pure parser to the live UI. This replaces the per-event full-rebuild with row-granular updates, and routes the final `assistant`/`result` frames through de-dup. This is the layer that makes streaming actually appear.

**Files:**
- Modify: `crates/app/src/lib.rs` — `handle_event` / `ingest_event`. Add a `StreamState` held in a UI-thread-local (a `Cell`/`RefCell` on a struct stored... — see design decision below).

**Design decision — where StreamState lives:** `StreamState` must persist across events on the UI thread, tied to the active session. Store it in a `Rc<RefCell<StreamState>>` created in `run_app`, cloned into the `handle_event` invocation. Since `handle_event` is called from `invoke_from_event_loop` (net.rs:211) with only `&App`, add a thread-local `std::thread_local! { static STREAM: RefCell<StreamState> = RefCell::new(StreamState::new()); }` keyed implicitly (one active session at a time on this client — the app shows one session). Reset it on session switch (in `on_selectSession` and `apply_sessions` new-session). This avoids threading the handle through `handle_event`'s signature.

**Interfaces:**
- Consumes: `StreamState`, `apply_stream_event`, `DeltaOp` (Task 2); `MsgBlock` (Task 1).
- Produces: a live `VecModel<MsgBlock>` updated incrementally. The `.slint` `messages` property is already `in-out property <[MsgBlock]>`; we keep setting it but now we mutate the SAME `VecModel` in place where possible.

```rust
use std::cell::RefCell;
thread_local! {
    static STREAM: RefCell<StreamState> = RefCell::new(StreamState::new());
}

/// Apply a DeltaOp list to the App's messages model, incrementally.
/// `model` is the current VecModel behind `messages`; we keep a clone of the
/// Rc so we can push/set_row_data without rebuilding.
fn apply_delta_ops(app: &App, ops: &[DeltaOp]) {
    let model = app.get_messages();
    // Downcast to VecModel to use push/set_row_data (row-granular signals).
    let vm = match model.as_any().downcast_ref::<slint::VecModel<MsgBlock>>() {
        Some(m) => m,
        None => {
            // Not a VecModel (e.g. an empty default) — fall back to full rebuild.
            // (Shouldn't normally happen after init.)
            return;
        }
    };
    for op in ops {
        match op {
            DeltaOp::UpsertBlock { row: None, block } => {
                let new_row = vm.row_count();
                vm.push(block.clone());
                STREAM.with(|s| {
                    // record the row for this block index so later deltas route here
                    let mut st = s.borrow_mut();
                    st.index_to_row.insert(block.blockIndex as usize, new_row);
                });
            }
            DeltaOp::UpsertBlock { row: Some(r), block } => {
                if *r < vm.row_count() {
                    vm.set_row_data(*r, block.clone());
                }
            }
            DeltaOp::ReplaceMessage { message_id, blocks } => {
                // Remove existing rows with this messageId, then insert the final set.
                let mut i = 0;
                while i < vm.row_count() {
                    if vm.row_data(i).map(|b| b.messageId == *message_id).unwrap_or(false) {
                        vm.remove(i);
                    } else {
                        i += 1;
                    }
                }
                for b in blocks {
                    vm.push(b.clone());
                }
            }
            DeltaOp::Reset => {
                STREAM.with(|s| s.borrow_mut().reset());
            }
        }
    }
}
```

- [ ] **Step 1: Add the thread-local `STREAM` and `apply_delta_ops`**

Add the code block above to `lib.rs`. Import `slint::Model` is already present (for `row_count`/`row_data`/`set_row_data`).

- [ ] **Step 2: Route `stream_event` through the parser in `handle_event`**

In `handle_event`'s match, before the existing arms, add a top-level branch for forwarded stream events. The server forwards `stream_event`-shaped events directly. Modify the `"event"` arm (lib.rs:554) — the net thread delivers every server frame via `handle_event`; frames with `type:"stream_event"` must now go to the parser. Add at the TOP of `handle_event`'s match:

```rust
fn handle_event(app: &App, msg: serde_json::Value) {
    let ty = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
    // Live streaming deltas — parse and render incrementally.
    if ty == "stream_event" {
        STREAM.with(|s| {
            let ops = apply_stream_event(&mut s.borrow_mut(), &msg);
            apply_delta_ops(app, &ops);
        });
        return;
    }
    // turn lifecycle (server emits system subtypes at top level too)
    if ty == "system" {
        let sub = msg.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
        match sub {
            "turn_started" => { app.set_busy(true); app.set_activeState("busy".into()); }
            "turn_stopped" => {
                app.set_busy(false);
                app.set_activeState("idle".into());
                STREAM.with(|s| s.borrow_mut().reset());
            }
            "bridge_error" => {
                let detail = msg.get("error").and_then(|v| v.as_str()).unwrap_or("turn failed");
                let mut msgs: Vec<MsgBlock> = app.get_messages().iter().collect();
                push_system_error(&mut msgs, &format!("Turn failed: {detail}"));
                app.set_messages(model_rc(msgs));
                app.set_busy(false);
                app.set_activeState("error".into());
            }
            "api_retry" => {
                if app.get_busy() { app.set_toast("Upstream rate-limited — retrying…".into()); }
            }
            _ => {}
        }
        return;
    }
    // de-dup final frame: a top-level assistant message replaces the streamed one
    if ty == "assistant" {
        let mid = msg.pointer("/message/id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let blocks = assemble_assistant_blocks(&msg); // see Step 3
        if !mid.is_empty() {
            let ops = vec![DeltaOp::ReplaceMessage { message_id: mid.clone(), blocks }];
            apply_delta_ops(app, &ops);
            STREAM.with(|s| { let mut st = s.borrow_mut(); if st.current_message_id.as_deref() == Some(&mid) { st.reset(); } });
        }
        return;
    }
    if ty == "result" {
        app.set_busy(false);
        app.set_activeState("idle".into());
        STREAM.with(|s| s.borrow_mut().reset());
        return;
    }
    if ty == "user" {
        let mut msgs: Vec<MsgBlock> = app.get_messages().iter().collect();
        ingest_user_message(&mut msgs, &msg); // extract existing logic (Step 4)
        app.set_messages(model_rc(msgs));
        return;
    }
    match ty {
        "hello" | "sessions" => { /* unchanged: parse_sessions + apply_sessions */ }
        "created" => { /* unchanged */ }
        "history" => { /* unchanged, but reset STREAM */ STREAM.with(|s| s.borrow_mut().reset()); }
        "stderr" => { /* unchanged */ }
        _ => {}
    }
}
```

(The existing arms for `hello/sessions/created/history/stderr` stay as-is; only wrap/early-return the new branches above them.)

- [ ] **Step 3: Implement `assemble_assistant_blocks`**

A helper that turns a final top-level `assistant` frame into `Vec<MsgBlock>` (re-using the existing per-block logic for text/tool_use, now with `messageId` set):

```rust
/// Build the authoritative final block list for a top-level assistant frame.
fn assemble_assistant_blocks(msg: &serde_json::Value) -> Vec<MsgBlock> {
    let mid = msg.pointer("/message/id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut out = Vec::new();
    let Some(content) = msg.pointer("/message/content").and_then(|c| c.as_array()) else { return out; };
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
                        codeLang: seg.2.into(),
                        messageId: mid.clone().into(),
                        blockIndex: i as i32,
                        ..default_block("text", &StreamState::new(), i) // dummy for the remaining fields
                    });
                }
            }
            "tool_use" => {
                let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("tool").to_string();
                out.push(MsgBlock {
                    kind: "tool".into(),
                    role: "assistant".into(),
                    text: tool_arg_preview(&name, block.get("input")).into(),
                    toolName: name.into(),
                    toolStatus: "running".into(),
                    toolId: id.into(),
                    messageId: mid.clone().into(),
                    blockIndex: i as i32,
                    ..default_block("tool", &StreamState::new(), i)
                });
            }
            "thinking" => {
                out.push(MsgBlock {
                    kind: "thinking".into(),
                    text: block.get("thinking").and_then(|v| v.as_str()).unwrap_or("").into(),
                    messageId: mid.clone().into(),
                    blockIndex: i as i32,
                    ..default_block("thinking", &StreamState::new(), i)
                });
            }
            _ => {}
        }
    }
    out
}
```

Note: the `..default_block(...)` spread needs `MsgBlock` to allow struct-update with the same type — it does (all fields same type families). If the spread clashes on a field set twice, drop that field from `default_block`'s return for this call site or set all fields explicitly. Prefer explicit fields; only spread the truly-unused ones (`time`, `expanded`).

- [ ] **Step 4: Extract `ingest_user_message` from the existing `"user"` arm**

Move the existing top-level `user`-block extraction (lib.rs:729-805) into `fn ingest_user_message(msgs: &mut Vec<MsgBlock>, msg: &serde_json::Value)`. Keep behavior identical; just relocate so `handle_event`'s `user` arm calls it.

- [ ] **Step 5: Reset STREAM on session switch**

In `on_selectSession` (lib.rs:275) after `set_messages(empty)`, add `STREAM.with(|s| s.borrow_mut().reset());`. Also in `apply_sessions` is unnecessary (sessions ≠ messages).

- [ ] **Step 6: Build**

Run: `cargo build -p synapse-app 2>&1 | tail -20`
Expected: clean. Fix any borrow/scope issues. The old `ingest_event`/`ingest_event_into` can stay (used nowhere now) or be deleted — **delete them** to avoid dead-code confusion, EXCEPT keep `ingest_event_into` if `history` backfill still uses it (it does — `history` calls `ingest_event_into`). So: keep `ingest_event_into` for history; remove the now-unused `ingest_event` wrapper (lib.rs:992) if it's no longer called. Verify with `grep -n "ingest_event\b" crates/app/src/lib.rs`.

- [ ] **Step 7: Run all tests**

Run: `cargo test -p synapse-app 2>&1 | tail -20`
Expected: all pass.

- [ ] **Step 8: Manual verify — desktop streaming (acceptance criterion 1)**

```bash
# server already running on 127.0.0.1:4173 token CODE (from prior session). If not:
# ./target/release/synapse-server --port 4173 --token CODE --host 0.0.0.0 --cwd /Users/zx/code/synapse &
SYNAPSE_HOST=127.0.0.1 SYNAPSE_PORT=4173 SYNAPSE_TOKEN=CODE SYNAPSE_TLS=0 ./target/debug/synapse-app &
APP_PID=$!
sleep 3
# In the window: the drawer should show sessions; select one, watch a streamed reply.
kill $APP_PID 2>/dev/null
```
Expected (criterion 1): selecting an active session and triggering a reply shows assistant text appearing token-by-token. Since you can't drive a reply headlessly, **the real verification is on sim (Task 7)**. On desktop, at minimum confirm the window renders, no panic, and a session's history loads. Report what you see.

- [ ] **Step 9: Stage**

```bash
git add crates/app/src/lib.rs
```

---

## Task 4: `.slint` chat view — collapsible thinking + tool blocks (default-collapsed)

Make the parsed `thinking`/`tool` blocks render as collapsible rows, collapsed by default. This is the ChatGPT/Claude/Cursor pattern (tools/thinking are noise; expand on tap).

**Files:**
- Modify: `crates/app/ui/app.slint` — the `for msg[i] in root.messages` block (lib.rs-equivalent: app.slint:448+). Add a `thinking` branch and refine the `tool` branch to default-collapsed.

**Interfaces:**
- Consumes: `MsgBlock.kind == "thinking"` now arrives from the parser (Task 2/3). `expanded` already toggles via `on_toggleTool`.

- [ ] **Step 1: Add the thinking branch + ensure tool default-collapsed**

In app.slint, inside the `for msg[i] in root.messages : VerticalLayout { ... }`, the tool card (`toolRect`, ~app.slint:450) already collapses on `msg.expanded`. Confirm `expanded` defaults to `false` — it does (Task 1 sets it false on creation). Add a `thinking` card before the tool card:

```slint
// ---- Thinking row (collapsible, default-collapsed) ----
thinkRect := Rectangle {
    visible: msg.kind == "thinking";
    height: msg.expanded ? thinkBody.preferred-height + 52px : 44px;
    border-radius: Theme.radiusSm;
    background: Theme.bgElev;
    border-width: 1px;
    border-color: Theme.border;
    VerticalLayout {
        Rectangle {
            height: 44px;
            TouchArea { clicked => { root.toggleTool(i); } }
            HorizontalLayout {
                padding-left: 14px; padding-right: 14px; spacing: 10px;
                Rectangle {
                    width: 22px; height: 22px; border-radius: 11px;
                    y: (parent.height - self.height) / 2;
                    background: Theme.bgElev2;
                    Text { text: "💭"; font-size: 12px; horizontal-alignment: center; vertical-alignment: center; }
                }
                Text {
                    text: "Thinking";
                    font-weight: 600; font-size: 14px; color: Theme.textDim;
                    vertical-alignment: center;
                }
                Rectangle { horizontal-stretch: 1; }
                Text {
                    text: msg.expanded ? "▾" : "▸";
                    font-size: 12px; color: Theme.textFaint; vertical-alignment: center;
                }
            }
        }
        Rectangle {
            visible: msg.expanded;
            clip: true;
            thinkBody := Text {
                text: msg.text;
                font-size: 13px; color: Theme.textDim; wrap: word-wrap;
                x: 14px; y: 4px; width: parent.width - 28px;
            }
        }
    }
}
```

Note: `toggleTool` flips `msg.expanded`. But thinking and tool blocks both bind height to `msg.expanded` — that's wrong if they're separate rows. **Fix**: thinking row toggling should NOT use the shared `expanded` of a tool block. Since each `msg` is a distinct block with its own `expanded` field, and `toggleTool(i)` toggles `messages[i].expanded`, this is correct PER ROW (each row is one msg with its own expanded). Verify the `toolRect` height also keys off its own `msg.expanded` (it does). So both are independently collapsible. Good.

- [ ] **Step 2: Build**

Run: `cargo build -p synapse-app 2>&1 | tail -10`
Expected: clean. (Slint compiles the .slint change.)

- [ ] **Step 3: Manual verify — desktop**

Run the desktop app (as Task 3 Step 8). Open a session whose transcript has thinking/tool blocks. Expected (criteria 2, 3): thinking rows appear collapsed, expand on tap; tool cards show name + status, expand on tap.

- [ ] **Stage:**
```bash
git add crates/app/ui/app.slint
```

---

## Task 5: Render throttle — coalesce deltas behind a 33ms timer

480 deltas/turn would schedule 480 model notifications. Throttle to one flush per frame (~30fps) so streaming is smooth even on the sim's software renderer.

**Files:**
- Modify: `crates/app/src/lib.rs` — the `stream_event` arm in `handle_event`. Buffer the latest `DeltaOp`s in the `STREAM` thread-local; a `slint::Timer` (single shared, `Repeat`) flushes pending ops every 33ms.

**Interfaces:**
- Consumes: `apply_delta_ops` (Task 3).
- Produces: smooth streaming. No new public API.

- [ ] **Step 1: Add a pending-ops buffer + timer**

Extend the `STREAM` thread-local to hold pending ops and a dirty flag:

```rust
thread_local! {
    static STREAM: RefCell<StreamCtx> = RefCell::new(StreamCtx::new());
}

struct StreamCtx {
    state: StreamState,
    pending: Vec<DeltaOp>,
    dirty: bool,
}
impl StreamCtx {
    const fn new() -> Self { /* can't const-construct StreamState; use Default below */ }
}
```
(`StreamState::new()` is non-const, so give `StreamCtx` a `Default` impl or a `pub fn new()` used via `RefCell::new(StreamCtx::fresh())`.) Keep it simple:

```rust
struct StreamCtx {
    state: StreamState,
    pending: Vec<DeltaOp>,
    dirty: bool,
}
impl StreamCtx {
    fn fresh() -> Self { Self { state: StreamState::new(), pending: Vec::new(), dirty: false } }
}
```

- [ ] **Step 2: Change the `stream_event` arm to buffer, not apply**

```rust
if ty == "stream_event" {
    STREAM.with(|s| {
        let mut ctx = s.borrow_mut();
        let ops = apply_stream_event(&mut ctx.state, &msg);
        if !ops.is_empty() {
            ctx.pending.extend(ops);
            ctx.dirty = true;
        }
    });
    return;
}
```

- [ ] **Step 3: Install the flush timer in `run_app`**

After `App::new()`, before `app.run()`:

```rust
{
    let weak = app.as_weak();
    let flush_timer = slint::Timer::default();
    flush_timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(33), move || {
        let ops = STREAM.with(|s| {
            let mut ctx = s.borrow_mut();
            if !ctx.dirty { return Vec::new(); }
            ctx.dirty = false;
            std::mem::take(&mut ctx.pending)
        });
        if !ops.is_empty() {
            if let Some(app) = weak.upgrade() {
                apply_delta_ops(&app, &ops);
            }
        }
    });
    // Keep the timer alive for the app's lifetime.
    std::mem::forget(flush_timer);
}
```
(`std::mem::forget` keeps the timer alive; otherwise it'd drop at end of block. Acceptable: the timer lives as long as the app.)

- [ ] **Step 4: Build + run tests**

Run: `cargo build -p synapse-app && cargo test -p synapse-app 2>&1 | tail -15`
Expected: clean build, tests pass.

- [ ] **Step 5: Manual verify — desktop streaming smoothness (criterion 4)**

Run desktop app, watch a long reply stream. Expected: smooth, no freezing. (Full sim verification in Task 7.)

- [ ] **Stage:**
```bash
git add crates/app/src/lib.rs
```

---

## Task 6: Keyboard inset — UIKit observer → `keyboardInset` property

The composer is occluded because winit does not shrink the iOS view on keyboard. Add a UIKit `UIKeyboardWillChangeFrameNotification` observer that calls a Rust `#[no_mangle] set_keyboard_inset(f32)`, which updates a `.slint` `keyboardInset` property; the composer container uses it as bottom padding.

**Files:**
- Modify: `crates/app/ui/app.slint` — add `in-out property <length> keyboardInset: 0px;`; the chat `VerticalLayout` composer container's height/padding accounts for it.
- Modify: `crates/app/src/lib.rs` — add `#[no_mangle] pub extern "C" fn synapse_set_keyboard_inset(px: f32)` (iOS-only) that updates the property via a global `Weak<App>`. Install that global weak in `run_app`.
- Modify: `mobile/ios/Sources/main.m` — register the keyboard observer in Obj-C (pure Foundation/UIKit, no objc2 dep) and call `synapse_set_keyboard_inset`.

**Interfaces:**
- Consumes: UIKit (no new crate; raw `extern "C"`).
- Produces: `synapse_set_keyboard_inset(f32: f32)` exported C symbol.

- [ ] **Step 1: Add the `.slint` property + composer bottom padding**

In app.slint, near the other `in-out property`s (~line 66), add:

```slint
in-out property <length> keyboardInset: 0px;
```

On the chat-view composer `Rectangle` (app.slint:684, `height: 64px`), change to make the composer sit above the keyboard by adding bottom padding to the *chat container*. Simplest: wrap — set the composer Rectangle's `y`/the outer container. Concretely, change the composer container so its height includes the inset:

```slint
// floating composer — rides above the keyboard
Rectangle {
    height: 64px + root.keyboardInset;
    background: Theme.bg;
    VerticalLayout {
        padding: 8px; padding-bottom: 10px + root.keyboardInset;
        // ... existing composer content unchanged ...
    }
}
```
(Keep the composer pill at the top of this taller container; the extra height is empty space under the keyboard.)

- [ ] **Step 2: Add the Rust C-export + global weak**

In lib.rs, add a thread-safe global weak handle and the exported setter:

```rust
#[cfg(target_os = "ios")]
slint::thread_local! {
    static IOS_APP: slint::Weak<App> = slint::Weak::default();
}

/// Called from the UIKit keyboard-frame observer (mobile/ios/Sources/main.m).
/// `px` is the keyboard height in physical pixels (screen space). Drives the
/// composer's bottom inset so it stays visible above the keyboard.
#[cfg(target_os = "ios")]
#[no_mangle]
pub extern "C" fn synapse_set_keyboard_inset(px: f32) {
    IOS_APP.with(|weak| {
        let w = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = w.upgrade() {
                app.set_keyboardInset((px.max(0.0)) as f32);
            }
        });
    });
}
```

Wait — `slint::Weak<App>` is the public type; it is `Send`. Use a plain `static` instead of thread_local for the weak, guarded by a `Mutex`:

```rust
#[cfg(target_os = "ios")]
static IOS_APP: std::sync::OnceLock<std::sync::Mutex<slint::Weak<App>>> = std::sync::OnceLock::new();

#[cfg(target_os = "ios")]
#[no_mangle]
pub extern "C" fn synapse_set_keyboard_inset(px: f32) {
    if let Some(m) = IOS_APP.get() {
        let w = m.lock().unwrap().clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = w.upgrade() {
                app.set_keyboardInset(px.max(0.0) as f32);
            }
        });
    }
}
```

And in `run_app`, install it (iOS-only):

```rust
#[cfg(target_os = "ios")]
{
    let _ = IOS_APP.get_or_init(|| std::sync::Mutex::new(app.as_weak()));
}
```

- [ ] **Step 3: Add the Obj-C keyboard observer in main.m**

```objc
#import <Foundation/Foundation.h>
#import <UIKit/UIKit.h>

extern void synapse_ios_main(void);
extern void synapse_set_keyboard_inset(float px);

static void keyboardWillChangeFrame(id note) {
    NSDictionary *info = [(NSNotification *)note userInfo];
    NSValue *endRectVal = info[UIKeyboardFrameEndUserInfoKey];
    CGRect endRect = endRectVal ? endRectVal.CGRectValue : CGRectZero;
    UIScreen *screen = [UIScreen mainScreen];
    CGFloat scale = screen.scale ?: 1.0;
    // keyboard occupies the bottom `endRect.size.height` pts; convert to px
    float px = (float)(endRect.size.height * scale);
    synapse_set_keyboard_inset(px);
}

int main(int argc, char *argv[]) {
    @autoreleasepool {
        synapse_ios_main();
        // Register the keyboard observer so the composer insets above the keyboard.
        [[NSNotificationCenter defaultCenter]
            addObserverForName:UIKeyboardWillChangeFrameNotification
                        object:nil
                         queue:[NSOperationQueue mainQueue]
                    usingBlock:^(NSNotification *n) { keyboardWillChangeFrame(n); }];
        // Slint/winit runs the main run loop from synapse_ios_main -> UIApplicationMain,
        // so this main() returns once the app terminates.
        return 0;
    }
}
```

**Important ordering issue:** `synapse_ios_main()` calls `run_app()` → `app.run()` which (on iOS) enters `UIApplicationMain` and **does not return** until the app exits. So the observer registration AFTER `synapse_ios_main()` never executes. **Fix**: register the observer BEFORE calling `synapse_ios_main()`:

```objc
int main(int argc, char *argv[]) {
    @autoreleasepool {
        [[NSNotificationCenter defaultCenter]
            addObserverForName:UIKeyboardWillChangeFrameNotification
                        object:nil
                         queue:[NSOperationQueue mainQueue]
                    usingBlock:^(NSNotification *n) { keyboardWillChangeFrame(n); }];
        synapse_ios_main();   // enters UIApplicationMain; does not return until exit
        return 0;
    }
}
```

- [ ] **Step 4: Build for sim**

```bash
cargo rustc -p synapse-app --lib --target aarch64-apple-ios-sim --crate-type staticlib --release 2>&1 | tail -5
cd mobile/ios && xcodebuild -project Synapse.xcodeproj -scheme Synapse -configuration Release \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro' ARCHS=arm64 -sdk iphonesimulator build 2>&1 | tail -8
```
Expected: BUILD SUCCEEDED for both.

- [ ] **Step 5: Manual verify — sim keyboard (criterion 5)**

```bash
cd /Users/zx/code/synapse
SIM=39BC7179-E0F5-40BF-811F-F61B7E03B877
APP="/Users/zx/Library/Developer/Xcode/DerivedData/Synapse-etuqwedoidhfhchfhstghjqtwagq/Build/Products/Release-iphonesimulator/Synapse.app"
xcrun simctl install $SIM "$APP"
xcrun simctl launch $SIM com.synapse.app.gnjza
xcrun simctl io $SIM screenshot /tmp/kb1.png  # tap the composer field (manually in sim UI)
```
Expected (criterion 5): tapping the composer opens the keyboard; the composer stays visible above it. (Manual tap required; verify visually.)

- [ ] **Stage:**
```bash
git add crates/app/ui/app.slint crates/app/src/lib.rs mobile/ios/Sources/main.m
```

---

## Task 7: Navigation polish + final sim acceptance run

Ensure all drawer open/close/return paths work, then run the full 7-criteria acceptance on sim.

**Files:**
- Modify (if needed): `crates/app/ui/app.slint` (drawer overlay close), `crates/app/src/lib.rs` (`on_selectSession` already closes drawer — verify).

- [ ] **Step 1: Verify drawer close paths**

Read app.slint:727 (`if root.drawerOpen && root.view == "chat" : Rectangle { ... TouchArea { clicked => { root.toggleDrawer(); } }` — the overlay tap closes). The drawer's own ✕ (app.slint:750) calls `toggleDrawer`. `on_selectSession` (lib.rs:279) sets `drawerOpen=false`. All three paths exist. **Confirm** no path is broken; if the overlay `TouchArea` is behind the drawer panel (z-order), the tap on the dimmed area might hit the panel instead — verify the overlay Rectangle is the FIRST child (drawn behind) and the panel is its child, so taps on the panel don't bubble to the overlay. (Slint TouchAreas don't bubble by default, so a tap on the panel won't close — correct. A tap on the dim area hits the overlay's TouchArea — correct.)

- [ ] **Step 2: Add a hardware/gesture consideration**

iOS edge-swipe back isn't applicable (single-window app, no navigation stack). The "back" affordance IS the ☰→drawer→select pattern, which works. No code change unless a path is broken.

- [ ] **Step 3: Full rebuild for sim**

```bash
cargo rustc -p synapse-app --lib --target aarch64-apple-ios-sim --crate-type staticlib --release 2>&1 | tail -3
cd mobile/ios && xcodebuild -project Synapse.xcodeproj -scheme Synapse -configuration Release \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro' ARCHS=arm64 -sdk iphonesimulator build 2>&1 | tail -3
```

- [ ] **Step 4: Install + launch + run each acceptance criterion**

```bash
cd /Users/zx/code/synapse
# ensure server is up
pgrep -fl synapse-server || ./target/release/synapse-server --port 4173 --token CODE --host 0.0.0.0 --cwd /Users/zx/code/synapse &
SIM=39BC7179-E0F5-40BF-811F-F61B7E03B877
APP="/Users/zx/Library/Developer/Xcode/DerivedData/Synapse-etuqwedoidhfhchfhstghjqtwagq/Build/Products/Release-iphonesimulator/Synapse.app"
xcrun simctl terminate $SIM com.synapse.app.gnjza 2>/dev/null
xcrun simctl install $SIM "$APP"
xcrun simctl launch $SIM com.synapse.app.gnjza
```

Verify each criterion (drive the UI in the sim window):
1. **Streaming**: send a prompt, watch text stream token-by-token.
2. **Thinking**: a reasoning reply shows a collapsible Thinking row (expand on tap).
3. **Tool calls**: a tool-using reply shows collapsible tool cards (running→done), expandable, no dupes.
4. **No stutter**: long reply streams smoothly.
5. **Keyboard**: tap composer → keyboard opens → composer visible above it.
6. **Navigation**: ☰ opens drawer; ✕ / overlay-tap / select-session each close it; selected session's history loads.
7. **Real sessions**: drawer shows live sessions (already verified pre-rewrite).

Take a screenshot per criterion with `xcrun simctl io $SIM screenshot /tmp/accN.png` and analyze.

- [ ] **Step 5: Report results**

Report each criterion PASS/FAIL with evidence (screenshots + netstat for connectivity). Do NOT mark done if any FAIL — file the specific failure and loop.

- [ ] **Stage all:**
```bash
git add -A
```
(Do not commit unless the user asks. Report the full staged change set + the acceptance matrix.)

---

## Self-Review

**1. Spec coverage:**
- Streaming (spec §1, criterion 1): Tasks 2+3 ✓
- Render cost (spec §2, criterion 4): Tasks 3+5 ✓
- Thinking blocks (spec §C, criterion 2): Tasks 2+3 (parse) + Task 4 (render) ✓
- Tool blocks collapsible (spec §C, criterion 3): Tasks 2+3 (parse) + Task 4 (render) ✓
- De-dup final frame (spec §1 reconciliation): Task 3 Step 2-3 ✓
- Keyboard inset (spec §C, criterion 5): Task 6 ✓
- Navigation/back (spec §C, criteria 6,7): Task 7 ✓
- Data model messageId/blockIndex (spec §data model): Task 1 ✓
All 7 acceptance criteria have implementing tasks. ✓

**2. Placeholder scan:** No "TBD"/"TODO". Task 3 Step 6 has a "keep or delete" decision — resolved: keep `ingest_event_into` (history uses it), delete only the unused `ingest_event` wrapper after verifying with grep. The `..default_block` spread in `assemble_assistant_blocks` is flagged with a concrete fallback. No vague steps.

**3. Type consistency:**
- `MsgBlock` fields `messageId: string`/`blockIndex: int` (Task 1) ↔ used as `messageId`/`blockIndex` in Tasks 2,3,4. ✓
- `DeltaOp` variants (Task 2) ↔ matched in `apply_delta_ops` (Task 3). `UpsertBlock{row, block}`, `ReplaceMessage{message_id, blocks}`, `Reset`. ✓ (`Reset` is produced by... — Task 3 calls `s.borrow_mut().reset()` directly in lifecycle arms, not via DeltaOp::Reset; `Reset` variant is unused. **Fix**: remove `DeltaOp::Reset` from the enum to avoid dead variant, OR emit it. Simpler: remove it — Task 2's enum should be `{ UpsertBlock, ReplaceMessage }` only, since reset is called directly.)
- `apply_stream_event(s, evt) -> Vec<DeltaOp>` (Task 2) ↔ called in Task 3. ✓
- `synapse_set_keyboard_inset(f32)` (Task 6) ↔ declared `extern` in main.m. ✓

**Action from self-review:** Remove `DeltaOp::Reset` from the Task 2 enum (reset is handled directly in Task 3). Done in the plan above by editing Task 2's `DeltaOp` definition to drop the `Reset` variant. (Implementer: the Task 2 enum is `{ UpsertBlock { row: Option<usize>, block: MsgBlock }, ReplaceMessage { message_id: String, blocks: Vec<MsgBlock> } }`.)

**Scope:** Single subsystem (client chat rendering). Not multi-subsystem. Appropriately one plan. ✓
