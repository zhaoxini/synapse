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
    { id: "s1", name: "UI 交互优化", cwd: "/workspace/synapse", state: "busy", started_at: now - 120000, model: "", pinned: true, archived: false, diff_adds: 42, diff_dels: 8 },
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

    overlayVisible === false || !(await page.locator("#connectOverlay").isVisible())
      ? ok("Overlay hides after connect") : fail("Overlay still visible after connect");

    await page.locator("body.mode-sessions").waitFor({ timeout: 3000 });
    ok("Session list view visible");

    !(await page.locator("body.mode-sessions #composer").isVisible())
      ? ok("No composer on session list") : fail("Composer shown on session list");

    (await page.locator(".s-time", { hasText: "Today" }).count()) > 0
      ? ok("Today section rendered") : fail("Today section missing");

    (await page.locator(".sess-sub.working").count()) > 0
      ? ok("Working status on busy session") : fail("Working status missing");

    const diffText = await page.locator(".sess-diff").first().textContent();
    diffText && diffText.includes("+")
      ? ok(`Diff stats shown (${diffText.trim()})`) : fail("Diff stats missing");

    (await page.locator(".sess-row.pinned").count()) > 0
      ? ok("Pinned session row styled") : fail("Pinned row missing");

    !(await page.locator("#workspaceBtn, #searchBtn, #refreshBtn, #selectBtn").count())
      ? ok("No clutter topbar buttons") : fail("Unexpected topbar buttons present");

    // chat + skeleton (use idle session so send isn't in stop mode)
    await page.locator(".sess-row").nth(1).click();
    const loading = await page.evaluate(() => document.getElementById("scroller").classList.contains("history-loading"));
    loading ? ok("History loading indicator") : fail("History loading indicator missing");
    await page.waitForTimeout(300);

    (await page.evaluate(() => document.body.classList.contains("mode-chat")))
      ? ok("Switches to chat mode on session tap") : fail("Chat mode not activated");

    (await page.locator("#composer").isVisible())
      ? ok("Composer visible in chat") : fail("Composer missing in chat");

    const sendDisabled = await page.locator("#sendBtn").isDisabled();
    sendDisabled ? ok("Send disabled when empty") : fail("Send should be disabled when empty");

    await page.locator("#backBtn").click();
    await page.waitForTimeout(200);
    (await page.evaluate(() => document.body.classList.contains("mode-sessions")))
      ? ok("Back returns to session list") : fail("Back did not return to list");

    (await page.locator("#newBtn").isVisible())
      ? ok("New session button visible") : fail("New session button missing");

    const title = await page.locator("#titleName").textContent();
    title && title.length > 0 ? ok(`Topbar title: "${title}"`) : fail("Empty topbar title");

    const dark = await page.evaluate(() => document.documentElement.classList.contains("theme-dark"));
    dark ? ok("Forced dark theme") : fail("Dark theme not applied");

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
