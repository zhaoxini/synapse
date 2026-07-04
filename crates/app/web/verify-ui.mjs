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

const suggestedCwds = ["/workspace/synapse", "/workspace/other"];
const registeredProjects = Array.from({ length: 20 }, (_, i) => `/workspace/manual-${String(i + 1).padStart(2, "0")}`);
registeredProjects.unshift("/workspace/manual-added");

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
        cwds: suggestedCwds,
        registeredProjects,
      }));
      ws.on("message", (raw) => {
        let msg; try { msg = JSON.parse(raw); } catch { return; }
        if (msg.op === "list" || msg.op === "refresh") {
          ws.send(JSON.stringify({ type: "sessions", sessions: mockSessions() }));
        } else if (msg.op === "refresh_cwds" || msg.op === "register_project") {
          ws.send(JSON.stringify({ type: "cwds", cwds: suggestedCwds, registeredProjects }));
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

    await page.locator("body.screen-workspaces").waitFor({ timeout: 3000 });
    ok("Workspaces view is default");

    (await page.locator("#pageTitle").textContent()) === "Workspaces"
      ? ok("Workspaces page title") : fail("Workspaces title missing");

    (await page.locator(".ws-row", { hasText: "All Repos" }).count()) >= 1
      ? ok("All Repos row present") : fail("All Repos row missing");

    (await page.locator(".ws-row").count()) >= 1
      ? ok("Workspace rows rendered") : fail("Workspace rows missing");

    {
      const mainUi = await page.evaluate(() => ({
        appH: document.getElementById("app")?.offsetHeight ?? 0,
        vh: window.innerHeight,
        dockH: document.getElementById("dockBar")?.offsetHeight ?? 0,
        listH: document.getElementById("workspaceList")?.offsetHeight ?? 0,
        dockBottomGap: (() => {
          const app = document.getElementById("app")?.getBoundingClientRect();
          const panel = document.getElementById("dockPanel")?.getBoundingClientRect();
          return app && panel ? Math.round(app.bottom - panel.bottom) : 999;
        })(),
        registeredVisible: document.getElementById("workspaceList")?.innerText.includes("manual-added") ?? false,
        suggestionVisible: document.getElementById("workspaceList")?.innerText.includes("other") ?? false,
      }));
      mainUi.appH >= mainUi.vh * 0.9
        ? ok("App fills viewport") : fail(`App too short (${mainUi.appH}/${mainUi.vh})`);
      mainUi.dockH > 40
        ? ok("Bottom dock visible") : fail("Bottom dock missing");
      mainUi.dockBottomGap <= 16
        ? ok("Bottom dock stays inside app bottom") : fail(`Bottom dock gap too large (${mainUi.dockBottomGap}px)`);
      mainUi.listH > 40
        ? ok("Workspace list visible") : fail("Workspace list collapsed");
      mainUi.registeredVisible
        ? ok("Registered repo shown on workspace list") : fail("Registered repo missing from workspace list");
      !mainUi.suggestionVisible
        ? ok("Suggested-only repos hidden from workspace list") : fail("Suggested-only repo leaked into workspace list");
    }

    !(await page.locator("#workspaceList .sess-row").count())
      ? ok("Sessions not inline on main list") : fail("Sessions should only appear on repo screen");

    await page.locator(".ws-row").nth(1).click();
    await page.waitForTimeout(200);
    (await page.locator("#screenRepo.active").count()) > 0
      ? ok("Repo screen opens on workspace tap") : fail("Repo screen missing");
    (await page.evaluate(() => document.body.classList.contains("screen-repo")))
      ? ok("Repo screen body class") : fail("Workspace tap should open repo screen");

    (await page.locator("#repoSessionList .sess-card").count()) > 0
      ? ok("Sessions shown on repo screen") : fail("Repo sessions missing");

    !(await page.locator("#repoSessionList .tree-new-row").count())
      ? ok("No new session row on repo screen") : fail("Repo should not show new session row");

    !(await page.locator("#composer").isVisible())
      ? ok("Composer hidden on workspaces list") : fail("Composer should be hidden on workspaces list");

    await page.locator("#repoBackBtn").click();
    await page.waitForTimeout(150);

    const sessBefore = await page.evaluate(() => window.__synapse.state.sessions.length);
    await page.locator("#newBtn").click();
    await page.waitForTimeout(200);
    (await page.evaluate(() => document.body.classList.contains("screen-workspaces")))
      ? ok("+ stays on workspaces list") : fail("+ should not leave workspaces list");
    (await page.locator("#bottomSheet.show").count()) > 0
      && (await page.locator("#sheetTitle").textContent()) === "Add Repository"
      ? ok("+ opens add repository sheet") : fail("+ should open add repository sheet");

    const addRepoUi = await page.evaluate(() => {
      const app = document.getElementById("app")?.getBoundingClientRect();
      const sheet = document.getElementById("bottomSheet")?.getBoundingClientRect();
      const title = document.getElementById("sheetTitle");
      const close = document.getElementById("sheetClose");
      const handle = document.getElementById("sheetHandle");
      const search = document.querySelector(".add-repo-search");
      const ts = title ? getComputedStyle(title) : null;
      const titleRect = title?.getBoundingClientRect();
      const closeRect = close?.getBoundingClientRect();
      return {
        sheetHeight: document.getElementById("bottomSheet")?.offsetHeight ?? 0,
        appLeft: app?.left ?? 0,
        sheetLeft: sheet?.left ?? 0,
        sheetWidth: sheet?.width ?? 0,
        appWidth: app?.width ?? 0,
        titleLeft: titleRect?.left ?? 0,
        closeRight: closeRect?.right ?? 0,
        sheetRight: sheet?.right ?? 0,
        titleFontSize: ts?.fontSize ?? "",
        titleFontWeight: ts?.fontWeight ?? "",
        titleAlign: ts?.textAlign ?? "",
        searchVisible: !!(search && search.offsetHeight > 10),
        handleW: handle?.offsetWidth ?? 0,
        handleH: handle?.offsetHeight ?? 0,
        hasAddRepoMask: document.getElementById("sheetMask")?.classList.contains("sheet-add-repo-mask"),
      };
    });
    addRepoUi.sheetHeight > 250
      ? ok("Add repo sheet tall enough") : fail(`Add repo sheet collapsed (${addRepoUi.sheetHeight}px)`);
    addRepoUi.titleLeft < (addRepoUi.closeRight - 40)
      ? ok("Add repo title left of close") : fail("Add repo title/close layout wrong");
    Math.abs(addRepoUi.closeRight - addRepoUi.sheetRight) < 24
      ? ok("Add repo close aligned right") : fail("Add repo close not on right edge");
    addRepoUi.titleFontSize === "17px" && Number(addRepoUi.titleFontWeight) >= 700
      ? ok("Add repo title typography") : fail("Add repo title not 17px bold");
    addRepoUi.titleAlign === "left"
      ? ok("Add repo title left-aligned") : fail("Add repo title should be left-aligned");
    addRepoUi.searchVisible
      ? ok("Add repo search visible") : fail("Add repo search missing");
    addRepoUi.handleW >= 38 && addRepoUi.handleH >= 4
      ? ok("Add repo handle size") : fail("Add repo handle wrong size");
    Math.abs(addRepoUi.sheetLeft - addRepoUi.appLeft) < 2
      && Math.abs(addRepoUi.sheetWidth - addRepoUi.appWidth) < 2
      ? ok("Add repo sheet within app frame") : fail("Add repo sheet not aligned to app frame");
    addRepoUi.hasAddRepoMask
      ? ok("Add repo light mask") : fail("Add repo mask class missing");

    (await page.evaluate(() => window.__synapse.state.sessions.length)) === sessBefore
      ? ok("No session added from +") : fail("+ prematurely created a session");
    await page.locator("#sheetClose").click();
    await page.waitForTimeout(150);

    await page.evaluate(() => {
      window.__synapse.startNewDraft("/workspace/synapse");
    });
    await page.waitForTimeout(200);
    (await page.evaluate(() => document.body.classList.contains("screen-chat")))
      ? ok("New session opens draft chat") : fail("New session should open draft chat");
    (await page.evaluate(() => window.__synapse.state.sessions.length)) === sessBefore
      ? ok("No session added until first message") : fail("New session prematurely created");
    (await page.locator("#empty .brand img[src='logo.svg']").count()) > 0
      ? ok("Synapse logo on empty state") : fail("Logo missing on empty state");
    await page.locator("#backBtn").click();
    await page.waitForTimeout(200);

    await page.locator(".ws-row").nth(1).click();
    await page.waitForTimeout(150);

    (await page.locator(".sess-icon.running").count()) > 0
      ? ok("Working session pulse dot") : fail("Running indicator missing");

    (await page.locator(".sess-sub.sess-branch .pulse-dot").count()) > 0
      ? ok("Pulse on busy session branch row") : fail("Pulse dot missing");

    (await page.locator(".sess-card-archive").count()) > 0
      ? ok("Swipe archive area on session cards") : fail("Archive swipe area missing");

    // chat + skeleton (use idle session)
    await page.locator("#repoSessionList .sess-card").nth(1).click();
    const loading = await page.evaluate(() => document.getElementById("scroller").classList.contains("history-loading"));
    loading ? ok("History loading indicator") : fail("History loading indicator missing");
    await page.waitForTimeout(300);

    (await page.evaluate(() => document.body.classList.contains("screen-chat")))
      ? ok("Switches to chat mode on session tap") : fail("Chat mode not activated");

    (await page.locator("#composerPanel").isVisible())
      ? ok("Chat composer panel visible") : fail("Chat composer missing");

    await page.locator("#composerCollapsedTap").click();
    await page.waitForTimeout(150);
    (await page.locator("#composerPanel.expanded").count()) > 0
      ? ok("Chat composer expands on tap") : fail("Chat composer should expand");

    (await page.evaluate(() => {
      const btn = document.getElementById("attachBtn");
      return btn ? btn.getBoundingClientRect().width : 999;
    })) <= 40
      ? ok("Attach button keeps compact size") : fail("Attach button stretched in composer");

    await page.locator("#composerDim").click();
    await page.waitForTimeout(100);

    const sendDisabled = (await page.locator("#sendBtn").count()) > 0;
    sendDisabled ? ok("Composer stop control in DOM") : fail("Stop button missing");

    await page.locator("#backBtn").click();
    await page.waitForTimeout(200);
    (await page.evaluate(() => document.body.classList.contains("screen-repo")))
      ? ok("Back on repo screen for dock test") : fail("Back should return to repo screen");
    await page.locator("#dockCollapsedTap").click();
    await page.waitForTimeout(150);
    (await page.locator("#dockPanel.expanded").count()) > 0
      ? ok("Bottom dock expands on tap") : fail("Dock should expand");
    await page.locator("#dockDim").click();
    await page.waitForTimeout(100);

    await page.locator("#repoSessionList .sess-card").nth(1).click();
    await page.waitForTimeout(300);
    await page.locator("#composerCollapsedTap").click();
    await page.waitForTimeout(150);

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
    (await page.evaluate(() => document.body.classList.contains("screen-repo")))
      ? ok("Back returns to repo session list") : fail("Back did not return to repo screen");

    await page.locator("#repoBackBtn").click();
    await page.waitForTimeout(200);
    (await page.evaluate(() => document.body.classList.contains("screen-workspaces")))
      ? ok("Back returns to workspaces list") : fail("Back did not return to workspaces list");

    const light = await page.evaluate(() => document.documentElement.classList.contains("theme-light"));
    light ? ok("Light theme applied") : fail("Light theme not applied");

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
