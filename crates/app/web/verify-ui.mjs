#!/usr/bin/env node
/**
 * Headless UI verification — loads the static bundle with a mock WS server.
 * Run: node verify-ui.mjs  (from crates/app/web, with static server on :8765)
 */
import { chromium } from "playwright";
import { WebSocketServer } from "ws";
import http from "http";

const STATIC = "http://127.0.0.1:8765";
const WS_PORT = 14173;
const issues = [];
const ok = (msg) => console.log(`  ✓ ${msg}`);
const fail = (msg) => { console.log(`  ✗ ${msg}`); issues.push(msg); };

function mockSessions() {
  const now = Date.now();
  return [
    { id: "s1", name: "UI 交互优化", cwd: "/workspace/synapse", state: "busy", started_at: now - 120000, model: "" },
    { id: "s2", name: "Development environment setup", cwd: "/workspace/synapse", state: "idle", started_at: now - 86400000, model: "" },
    { id: "s3", name: "Failed run", cwd: "/workspace/synapse", state: "error", started_at: now - 172800000, model: "" },
  ];
}

function startMockWs() {
  return new Promise((resolve) => {
    const wss = new WebSocketServer({ port: WS_PORT });
    wss.on("connection", (ws) => {
      ws.send(JSON.stringify({
        type: "hello",
        sessions: mockSessions(),
        models: [{ id: "sonnet", label: "Sonnet" }],
        defaultModel: "sonnet",
        cwds: ["/workspace/synapse"],
      }));
      ws.on("message", (raw) => {
        let msg; try { msg = JSON.parse(raw); } catch { return; }
        if (msg.op === "list") {
          ws.send(JSON.stringify({ type: "sessions", sessions: mockSessions() }));
        } else if (msg.op === "history") {
          ws.send(JSON.stringify({ type: "history", sessionId: msg.sessionId, events: [], found: true }));
        } else if (msg.op === "create") {
          ws.send(JSON.stringify({
            type: "created",
            session: { id: "s-new", name: "New session", cwd: "/workspace/synapse", state: "idle", started_at: Date.now() },
          }));
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
    // --- no creds: pairing overlay ---
    await page.goto(STATIC);
    const overlayVisible = await page.locator("#connectOverlay").isVisible();
    overlayVisible ? ok("Pairing overlay shown without creds") : fail("Pairing overlay missing");

    const composerHidden = !(await page.locator("#composer").isVisible());
    composerHidden ? ok("Composer hidden on pairing screen") : fail("Composer visible during pairing");

    // --- connect with mock ---
    await page.evaluate((port) => {
      localStorage.setItem("synapse_creds", JSON.stringify({
        host: "127.0.0.1", port: String(port), token: "TEST", tls: false, path: "",
      }));
    }, WS_PORT);
    await page.reload();
    await page.waitForTimeout(800);

    const overlayGone = !(await page.locator("#connectOverlay").isVisible());
    overlayGone ? ok("Overlay hides after connect") : fail("Overlay still visible after connect");

    const sessionView = await page.locator("#sessionView").isVisible();
    sessionView ? ok("Session list view visible") : fail("Session list not visible");

    const composerOnList = await page.locator("body.mode-sessions #composer").isVisible();
    !composerOnList ? ok("No composer on session list") : fail("Composer shown on session list");

    const todayHeader = await page.locator(".s-time", { hasText: "Today" }).count();
    todayHeader > 0 ? ok("Today section rendered") : fail("Today section missing");

    const working = await page.locator(".sess-sub.working").count();
    working > 0 ? ok("Working status on busy session") : fail("Working status missing");

    // --- open chat ---
    await page.locator(".sess-row").first().click();
    await page.waitForTimeout(300);

    const chatMode = await page.evaluate(() => document.body.classList.contains("mode-chat"));
    chatMode ? ok("Switches to chat mode on session tap") : fail("Chat mode not activated");

    const composerInChat = await page.locator("#composer").isVisible();
    composerInChat ? ok("Composer visible in chat") : fail("Composer missing in chat");

    const backVisible = await page.locator("#backBtn").isVisible();
    backVisible ? ok("Back button visible in chat") : fail("Back button missing in chat");

    // --- back to list ---
    await page.locator("#backBtn").click();
    await page.waitForTimeout(200);
    const listAgain = await page.evaluate(() => document.body.classList.contains("mode-sessions"));
    listAgain ? ok("Back returns to session list") : fail("Back did not return to list");

    const composerGone = !(await page.locator("body.mode-sessions #composer").isVisible());
    composerGone ? ok("Composer hidden after back") : fail("Composer still visible after back");

    // --- search toggle ---
    await page.locator("#searchBtn").click();
    const searchOpen = await page.locator("#searchWrap:not(.hidden)").isVisible();
    searchOpen ? ok("Search bar toggles open") : fail("Search toggle broken");

    // --- layout: topbar title not empty ---
    const title = await page.locator("#titleName").textContent();
    title && title.length > 0 ? ok(`Topbar title: "${title}"`) : fail("Empty topbar title");

    // --- tap targets ---
    const rowBox = await page.locator(".sess-row").first().boundingBox();
    rowBox && rowBox.height >= 44 ? ok(`Session row height ${Math.round(rowBox.height)}px`) : fail("Session row too short (<44px)");

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
