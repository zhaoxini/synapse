import { IonApp } from "@ionic/react";
import { useEffect } from "react";

/** Synapse UI shell — Ionic iOS wrapper around the imperative chat core (synapse-core.js). */
export default function App() {
  useEffect(() => {
    if (document.querySelector('script[data-synapse-core]')) return;
    const s = document.createElement("script");
    s.src = "./synapse-core.js";
    s.dataset.synapseCore = "1";
    document.body.appendChild(s);
  }, []);

  return (
    <IonApp className="synapse-app">
      <header id="appHeader">
        <div id="topbar">
          <div className="topbar-left">
            <button id="backBtn" className="iconbtn iconbtn-glass" aria-label="Back" hidden>
              <svg width="18" height="18" viewBox="0 0 20 20" fill="none">
                <path d="M12.5 4.5L7 10l5.5 5.5" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
              </svg>
            </button>
          </div>
          <div id="chatTitle" className="chat-title" hidden>
            Chat
          </div>
        </div>

        <div id="wsToolbar" className="ws-toolbar">
          <button type="button" id="profileBtn" className="iconbtn iconbtn-glass profile-btn" aria-label="Profile">
            <img src="logo.svg" width="20" height="20" alt="" />
          </button>
          <div className="ws-toolbar-actions">
            <button id="searchBtn" className="iconbtn iconbtn-glass" aria-label="Search">
              <svg width="17" height="17" viewBox="0 0 20 20" fill="none">
                <circle cx="9" cy="9" r="5.5" stroke="currentColor" strokeWidth="1.7" />
                <path d="M13.5 13.5L17 17" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" />
              </svg>
            </button>
            <button id="newBtn" className="iconbtn iconbtn-glass" aria-label="Add workspace">
              <svg width="17" height="17" viewBox="0 0 20 20" fill="none">
                <path d="M10 4v12M4 10h12" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" />
              </svg>
            </button>
          </div>
        </div>

        <div id="pageHead">
          <h1 id="pageTitle">Workspaces</h1>
        </div>

        <div id="searchWrap" className="hidden">
          <div className="search-field">
            <svg className="search-field-icon" width="16" height="16" viewBox="0 0 20 20" fill="none" aria-hidden="true">
              <circle cx="9" cy="9" r="5.5" stroke="currentColor" strokeWidth="1.6" />
              <path d="M13.5 13.5L17 17" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
            </svg>
            <input id="searchInput" type="search" placeholder="Search" autoComplete="off" />
          </div>
        </div>
      </header>

      <div id="reconnect">Reconnecting…</div>
      <div id="toast">
        <span id="toastText" />
        <button type="button" id="toastClose">
          ✕
        </button>
      </div>

      <section id="workspaceView">
        <div id="pullRefresh" className="hidden" aria-hidden="true">
          <span className="pull-spin" />
          <span className="pull-label">Release to refresh</span>
        </div>
        <div id="workspaceList" />
        <button type="button" id="archivedToggle" className="archived-toggle hidden">
          Show archived
        </button>
      </section>

      <div id="chatView">
        <main id="scroller">
          <div id="messages" />
        </main>
        <button type="button" id="newPill">
          ↓ New
        </button>
        <div id="empty">
          <div className="empty-inner">
            <div className="brand" aria-hidden="true">
              <img src="logo.svg" width="56" height="56" alt="" />
            </div>
            <h1>Let&apos;s build</h1>
            <p id="emptySub" />
            <div className="suggestions">
              <button type="button" data-prompt="Summarize the recent changes in this repo.">
                Summarize recent changes
              </button>
              <button type="button" data-prompt="Explain what this workspace does.">
                Explain this workspace
              </button>
              <button type="button" data-prompt="Find and fix any failing tests.">
                Find and fix failing tests
              </button>
            </div>
          </div>
        </div>
      </div>

      <footer id="composer">
        <div id="composerDock">
          <div id="composerRow">
            <div className="composer-field">
              <button id="attachBtn" className="dock-btn dock-plus" aria-label="More">
                <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                  <path d="M8 3v10M3 8h10" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
                </svg>
              </button>
              <textarea
                id="input"
                rows={1}
                  placeholder="Plan, ask, build..."
                autoComplete="off"
                autoCorrect="on"
                autoCapitalize="sentences"
                spellCheck
                enterKeyHint="send"
                aria-autocomplete="none"
                data-1p-ignore=""
                data-lpignore="true"
                data-form-type="other"
                name="synapse-message"
                readOnly
              />
              <button type="button" id="micBtn" className="dock-btn dock-mic" aria-label="Voice input">
                <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                  <rect x="5.5" y="2" width="5" height="8" rx="2.5" stroke="currentColor" strokeWidth="1.4" />
                  <path d="M3.5 8a4.5 4.5 0 009 0M8 12.5V14" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
                </svg>
              </button>
            </div>
            <div id="composerControls">
              <button type="button" className="dock-btn dock-text" id="modelCtl">
                <span id="modelLabel">Auto</span>
              </button>
              <span className="spacer" />
              <button type="button" id="sendBtn" aria-label="Send">
                <svg className="ico-send" width="16" height="16" viewBox="0 0 16 16" fill="none">
                  <path d="M8 12V4M8 4L4.5 7.5M8 4l3.5 3.5" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
                </svg>
                <svg className="ico-stop" width="12" height="12" viewBox="0 0 12 12" fill="none">
                  <rect x="2" y="2" width="8" height="8" rx="1" fill="currentColor" />
                </svg>
              </button>
            </div>
          </div>
        </div>
        <div id="attachMenu" className="model-menu" />
      </footer>

      <div id="localMenu" className="model-menu" />

      <div id="drawerMask" />
      <aside id="sessionDrawer" aria-hidden="true">
        <header id="drawerHead">
          <button type="button" id="drawerClose" className="iconbtn iconbtn-glass" aria-label="Close">
            <svg width="16" height="16" viewBox="0 0 20 20" fill="none">
              <path d="M5 5l10 10M15 5L5 15" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
            </svg>
          </button>
          <h2 id="drawerTitle" />
        </header>
        <div id="drawerBody" />
      </aside>

      <div id="sheetMask" />
      <div id="bottomSheet">
        <div id="sheetHandle" />
        <div id="sheetHead">
          <button type="button" id="sheetClose" aria-label="Close">
            ✕
          </button>
          <span id="sheetTitle" />
        </div>
        <div id="sheetBody" />
      </div>

      <div id="connectOverlay">
        <div className="connect-card">
          <h2>Connect to Claude Code</h2>
          <p className="connect-hint">
            Paste the <code>synapse://</code> link from the terminal where <code>synapse-server</code> is running.
          </p>
          <textarea id="pairLink" rows={3} placeholder="synapse://192.168.1.10:4173?token=CODE&tls=0" autoComplete="off" />
          <button type="button" id="pairConnect" className="connect-btn">
            Connect
          </button>
          <details className="connect-manual">
            <summary>Manual connection</summary>
            <label>
              Host <input id="pairHost" type="text" placeholder="192.168.1.10" autoComplete="off" />
            </label>
            <label>
              Port <input id="pairPort" type="text" placeholder="4173" inputMode="numeric" />
            </label>
            <label>
              Token <input id="pairToken" type="text" placeholder="CODE" autoComplete="off" />
            </label>
            <label className="pair-tls">
              <input id="pairTls" type="checkbox" /> Secure (wss / TLS)
            </label>
            <button type="button" id="pairManualConnect" className="connect-btn secondary">
              Connect manually
            </button>
          </details>
        </div>
      </div>
    </IonApp>
  );
}
