#!/usr/bin/env node
/**
 * Verify chat-only native shell mode (?shell=native).
 */
import { chromium } from "playwright";
import { WebSocketServer } from "ws";

const STATIC = "http://127.0.0.1:8765";
const WS_PORT = 14174;
const issues = [];
const ok = (msg) => console.log(`  ✓ ${msg}`);
const fail = (msg) => { console.log(`  ✗ ${msg}`); issues.push(msg); };

function startMockWs() {
  return new Promise((resolve) => {
    const wss = new WebSocketServer({ port: WS_PORT });
    wss.on("connection", (ws) => {
      const now = Date.now();
      ws.send(JSON.stringify({
        type: "hello",
        sessions: [
          { id: "s1", name: "Test chat", cwd: "/workspace/synapse", state: "idle", started_at: now },
        ],
        models: [{ id: "sonnet", label: "Sonnet" }],
        defaultModel: "sonnet",
        cwds: ["/workspace/synapse"],
      }));
      ws.on("message", (raw) => {
        let msg; try { msg = JSON.parse(raw); } catch { return; }
        if (msg.op === "history") {
          ws.send(JSON.stringify({ type: "history", sessionId: msg.sessionId, events: [], found: true }));
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
    await page.goto(`${STATIC}/?shell=native&host=127.0.0.1&port=${WS_PORT}&token=TEST&tls=0`);
    await page.waitForFunction(() => window.__synapse?.state, { timeout: 8000 });
    await page.waitForTimeout(400);

    (await page.locator("body.mode-native-shell").count()) > 0
      ? ok("Native shell body class") : fail("mode-native-shell missing");

    !(await page.locator("#workspaceView").isVisible())
      ? ok("Workspace view hidden") : fail("Workspace should be hidden in native shell");

    (await page.locator("#chatView").isVisible())
      ? ok("Chat view visible") : fail("Chat view should be visible");

    await page.evaluate(() => window.__synapse.openSession("s1"));
    await page.waitForTimeout(300);
    (await page.evaluate(() => window.__synapse.state.activeId)) === "s1"
      ? ok("openSession selects session") : fail("openSession failed");

    (await page.locator("#composer").isVisible())
      ? ok("Composer visible in native shell") : fail("Composer missing");
  } finally {
    await browser.close();
    stopWs();
  }
  if (issues.length) {
    console.log(`\nFAILED: ${issues.length}`);
    process.exit(1);
  }
  console.log("\nNative shell checks passed.");
}

main().catch((e) => { console.error(e); process.exit(1); });
