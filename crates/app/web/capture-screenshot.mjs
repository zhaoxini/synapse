#!/usr/bin/env node
/**
 * Capture mobile viewport screenshots for visual verification.
 * Requires static server on :8765 and mock WS on :14173 (same as verify-ui.mjs).
 */
import { chromium } from "playwright";
import { WebSocketServer } from "ws";
import fs from "fs";
import path from "path";

const STATIC = "http://127.0.0.1:8765";
const WS_PORT = 14173;
const OUT = process.env.SYNAPSE_SCREENSHOT_DIR || "/opt/cursor/artifacts/screenshots";

function mockSessions() {
  const now = Date.now();
  return [
    { id: "s1", name: "Development environment setup", cwd: "/workspace/synapse", state: "busy", started_at: now, pinned: false, archived: false, diff_adds: 0, diff_dels: 0 },
    { id: "s2", name: "Ui 交互优化", cwd: "/workspace/synapse", state: "error", started_at: now - 3600000, pinned: false, archived: false, diff_adds: 1063, diff_dels: 207 },
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
        if (msg.op === "history") {
          ws.send(JSON.stringify({
            type: "history",
            sessionId: msg.sessionId,
            events: [
              { type: "user", message: { content: [{ type: "text", text: "hi" }] } },
              { type: "assistant", message: { content: [{ type: "text", text: "Hi! How can I help?" }] } },
            ],
            found: true,
          }));
        }
      });
    });
    resolve(() => wss.close());
  });
}

async function main() {
  fs.mkdirSync(OUT, { recursive: true });
  const stopWs = await startMockWs();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 393, height: 852 } });

  try {
    await page.goto(STATIC);
    await page.evaluate((port) => {
      localStorage.setItem("synapse_creds", JSON.stringify({
        host: "127.0.0.1", port: String(port), token: "TEST", tls: false, path: "",
      }));
    }, WS_PORT);
    await page.reload();
    await page.waitForTimeout(600);
    await page.screenshot({ path: path.join(OUT, "01-workspaces.png"), fullPage: false });

    await page.locator(".ws-tree-children .sess-row").first().click();
    await page.waitForTimeout(500);
    await page.screenshot({ path: path.join(OUT, "02-chat.png"), fullPage: false });

    await page.locator("#backBtn").click();
    await page.waitForTimeout(400);
    await page.screenshot({ path: path.join(OUT, "03-tree.png"), fullPage: false });

    console.log(`Screenshots saved to ${OUT}`);
  } finally {
    await browser.close();
    stopWs();
  }
}

main().catch((e) => { console.error(e); process.exit(1); });
