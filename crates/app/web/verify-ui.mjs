#!/usr/bin/env node
/**
 * Headless UI verification — loads the static bundle with a mock WS server.
 * Run: node verify-ui.mjs  (from crates/app/web, with static server on :8765)
 */
import { chromium } from "playwright";
import { WebSocketServer } from "ws";

const STATIC = "http://127.0.0.1:8765";
const WS_PORT = 14173;
const issues = [];
const ok = (msg) => console.log(`  ✓ ${msg}`);
const fail = (msg) => { console.log(`  ✗ ${msg}`); issues.push(msg); };

let sessions = [];

function mockSessions() {
  const now = Date.now();
  return sessions.length ? sessions : [
    { id: "s1", name: "Ui 交互优化", cwd: "/workspace/synapse", state: "busy", started_at: now - 120000, model: "", pinned: true, archived: false, diff_adds: 1063, diff_dels: 207 },
    { id: "s2", name: "Development environment setup", cwd: "/workspace/synapse", state: "idle", started_at: now - 86400000, model: "", pinned: false, archived: false, diff_adds: 12, diff_dels: 3 },
    { id: "s3", name: "Failed run", cwd: "/workspace/synapse", state: "error", started_at: now - 172800000, model: "", pinned: false, archived: true, diff_adds: 0, diff_dels: 0 },
  ];
}

function startMockWs() {
  return new Promise((resolve) => {
    const wss = new WebSocketServer({ port: WS_PORT });
    wss.on("connection", (ws) => {
      sessions = mockSessions();
      ws.send(JSON.stringify({
        type: "hello",
        sessions,
        models: [{ id: "sonnet", label: "Sonnet" }],
        defaultModel: "sonnet",
        cwds: ["/workspace/synapse", "/workspace/other"],
      }));
      ws.on("message", (raw) => {
        let msg; try { msg = JSON.parse(raw); } catch { return; }
        if (msg.op === "list" || msg.op === "refresh") {
          ws.send(JSON.stringify({ type: "sessions", sessions: mockSessions() }));
        } else if (msg.op === "refresh_cwds") {
          ws.send(JSON.stringify({ type: "cwds", cwds: ["/workspace/synapse", "/workspace/other"] }));
        } else if (msg.op === "history") {
          setTimeout(() => {
            ws.send(JSON.stringify({ type: "history", sessionId: msg.sessionId, events: [], found: true }));
          }, 120);
        } else if (msg.op === "create") {
          ws.send(JSON.stringify({
            type: "created",
            session: { id: "s-new", name: "New session", cwd: "/workspace/synapse", state: "idle", started_at: Date.now() },
          }));
        } else if (msg.op === "pin") {
          const s = sessions.find(x => x.id === msg.sessionId);
          if (s) {
            s.pinned = msg.pinned !== false;
            ws.send(JSON.stringify({ type: "event", event: { type: "system", subtype: "session_updated", sessionId: s.id, session: s } }));
          }
        } else if (msg.op === "archive") {
          const ids = msg.sessionIds || [msg.sessionId];
          for (const id of ids) {
            const s = sessions.find(x => x.id === id);
            if (s) {
              s.archived = true;
              ws.send(JSON.stringify({ type: "event", event: { type: "system", subtype: "session_updated", sessionId: s.id, session: s } }));
            }
          }
        }
      });
    });
    resolve(() => wss.close());
  });
}

async function main() {
  const stopWs = await startMockWs();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 393, height: 852 } });

  try {
    await page.goto(STATIC);
    const overlayVisible = await page.locator("#connectOverlay").isVisible();
    overlayVisible ? ok("Pairing overlay shown without creds") : fail("Pairing overlay missing");

    await page.evaluate((port) => {
      localStorage.setItem("synapse_creds", JSON.stringify({
        host: "127.0.0.1", port: String(port), token: "TEST", tls: false, path: "",
      }));
    }, WS_PORT);
    await page.reload();
    await page.waitForTimeout(800);

    !(await page.locator("#connectOverlay").isVisible())
      ? ok("Overlay hides after connect") : fail("Overlay still visible after connect");

    await page.locator("body.mode-workspaces").waitFor({ timeout: 3000 });
    ok("Workspaces view is default");

    (await page.locator("#pageTitle").textContent()) === "Workspaces"
      ? ok("Workspaces page title") : fail("Workspaces title missing");

    (await page.locator(".ws-row").count()) >= 3
      ? ok("Workspace rows rendered") : fail("Workspace rows missing");

    (await page.locator("#composer").isVisible())
      ? ok("Composer visible on workspaces") : fail("Composer missing on workspaces");

    (await page.locator("#input").getAttribute("placeholder"))?.includes("Plan")
      ? ok("List composer placeholder") : fail("Wrong composer placeholder");

    const sessBefore = await page.evaluate(() => window.__synapse.state.sessions.length);
    await page.locator("#newBtn").click();
    await page.waitForTimeout(200);
    (await page.evaluate(() => document.body.classList.contains("mode-chat")))
      ? ok("+ opens draft chat") : fail("+ should open draft chat, not create session");
    (await page.evaluate(() => window.__synapse.state.sessions.length)) === sessBefore
      ? ok("No session added until first message") : fail("+ prematurely created a session");
    (await page.locator("#empty .brand img[src='logo.svg']").count()) > 0
      ? ok("Synapse logo on empty state") : fail("Logo missing on empty state");
    await page.locator("#backBtn").click();
    await page.waitForTimeout(200);
    await page.locator("#backBtn").click();
    await page.waitForTimeout(200);

    await page.locator(".ws-row", { hasText: "synapse" }).click();
    await page.waitForTimeout(200);
    (await page.evaluate(() => document.body.classList.contains("mode-sessions")))
      ? ok("Opens session list for repo") : fail("Session list not opened");

    (await page.locator("#pageTitle").textContent()) === "synapse"
      ? ok("Repo title shown") : fail("Repo title wrong");

    (await page.locator(".s-time", { hasText: "Today" }).count()) > 0
      ? ok("Today section rendered") : fail("Today section missing");

    (await page.locator(".sess-icon.spark").count()) > 0
      ? ok("Working session sparkle icon") : fail("Sparkle icon missing");

    (await page.locator(".sess-sub.working").count()) > 0
      ? ok("Working status on busy session") : fail("Working status missing");

    (await page.locator(".sess-archive-btn").count()) > 0
      ? ok("Inline archive button on rows") : fail("Archive button missing");

    // chat + skeleton (use idle session)
    await page.locator(".sess-row").nth(1).click();
    const loading = await page.evaluate(() => document.getElementById("scroller").classList.contains("history-loading"));
    loading ? ok("History loading indicator") : fail("History loading indicator missing");
    await page.waitForTimeout(300);

    (await page.evaluate(() => document.body.classList.contains("mode-chat")))
      ? ok("Switches to chat mode on session tap") : fail("Chat mode not activated");

    (await page.locator("#composerControls").isVisible())
      ? ok("Expanded composer in chat") : fail("Chat composer controls missing");

    const sendDisabled = await page.locator("#sendBtn").isDisabled();
    sendDisabled ? ok("Send disabled when empty") : fail("Send should be disabled when empty");

    await page.locator("#modelCtl").click();
    await page.waitForTimeout(150);
    (await page.locator("#sheetTitle").textContent()) === "Model"
      ? ok("Model picker sheet opens") : fail("Model sheet title wrong");
    (await page.locator(".model-section", { hasText: "Active" }).count()) > 0
      ? ok("Model Active section") : fail("Model Active section missing");
    (await page.locator(".model-search").isVisible())
      ? ok("Model search bar") : fail("Model search missing");
    (await page.locator(".model-row", { hasText: "Sonnet" }).count()) > 0
      ? ok("Model list from server") : fail("Model rows missing");
    await page.locator("#sheetClose").click();

    // thinking stream → sheet content
    await page.evaluate(() => {
      const { handleEvent } = window.__synapse;
      handleEvent({ type: "system", subtype: "turn_started", sessionId: "s2" });
      handleEvent({
        type: "stream_event", sessionId: "s2",
        event: { type: "message_start", message: { id: "msg_think", role: "assistant", content: [] } },
      });
      handleEvent({
        type: "stream_event", sessionId: "s2",
        event: { type: "content_block_start", index: 0, content_block: { type: "thinking", thinking: "" } },
      });
      handleEvent({
        type: "stream_event", sessionId: "s2",
        event: { type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: "Analyzing the UI layout." } },
      });
    });
    await page.waitForTimeout(150);
    await page.locator(".status-line", { hasText: "Thought" }).first().click();
    const thoughtBody = await page.locator("#sheetBody .sheet-thinking").textContent();
    thoughtBody && thoughtBody.includes("Analyzing")
      ? ok("Thinking sheet shows streamed content") : fail(`Thinking content missing (${thoughtBody})`);
    await page.locator("#sheetClose").click();
    await page.evaluate(() => {
      window.__synapse.handleEvent({ type: "system", subtype: "turn_stopped", sessionId: "s2" });
    });

    await page.locator("#backBtn").click();
    await page.waitForTimeout(200);
    (await page.evaluate(() => document.body.classList.contains("mode-sessions")))
      ? ok("Back returns to session list") : fail("Back did not return to list");

    await page.locator("#backBtn").click();
    await page.waitForTimeout(200);
    (await page.evaluate(() => document.body.classList.contains("mode-workspaces")))
      ? ok("Back returns to workspaces") : fail("Back did not return to workspaces");

    const light = await page.evaluate(() => document.documentElement.classList.contains("theme-light"));
    light ? ok("Light theme applied") : fail("Light theme not applied");

    // empty transcript keeps the draft empty state (no flash regression)
    await page.locator(".ws-row", { hasText: "synapse" }).click();
    await page.waitForTimeout(150);
    await page.locator(".sess-row").nth(1).click();
    await page.waitForTimeout(400);
    const emptyVisible = await page.evaluate(() => !document.getElementById("empty").classList.contains("hidden"));
    emptyVisible ? ok("Empty state after blank history") : fail("Empty state missing after blank history");

    await page.evaluate(() => {
      const z = parseInt(getComputedStyle(document.getElementById("newPill")).zIndex, 10);
      if (!(z >= 10)) throw new Error("low z-index");
    });
    ok("New pill has stacking z-index");

  } finally {
    await browser.close();
    stopWs();
  }

  console.log("\n---");
  if (issues.length) {
    console.log(`FAILED: ${issues.length} issue(s)`);
    process.exit(1);
  }
  console.log("All UI checks passed.");
}

main().catch((e) => { console.error(e); process.exit(1); });
