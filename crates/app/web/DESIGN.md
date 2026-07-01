# Synapse Web UI — Design System

Mobile web chat UI embedded in iOS WKWebView (`crates/app/web/`). Visual target: **Cursor mobile light theme** with **iOS interaction patterns** (spacing, touch targets, sheets, search bar).

This document is the source of truth for tokens, component structure, and layout rules. Implementation lives in `app.css` + `index.html` + `app.js`.

---

## Hybrid architecture (iOS native shell + web chat)

Like Cursor / ChatGPT mobile:

| Layer | Technology | Responsibility |
|-------|------------|----------------|
| **Shell** | SwiftUI (`mobile/ios/Sources/`) | Workspaces list, session sheet, navigation, system animations, haptics |
| **Chat** | WKWebView + `crates/app/web/?shell=native` | Transcript, composer, streaming markdown, tool cards |
| **Browser / Pages** | Full web bundle (default) | Same UI as before for GitHub Pages and dev |

Native shell holds its own WebSocket for session/workspace state. Chat webview opens a second WS connection for the active session (server broadcasts to all clients).

**Bridge:** `window.__synapse.openSession(id)` ← native; `chatReady` / haptic / copy → native via `webkit.messageHandlers.synapse`.

**Build:** `./mobile/build-sim.sh` on macOS (iOS 16+). Web-only verification: `./scripts/verify-web.sh` + `verify-native-shell.mjs`.

---

## Domain model (workspaces & sessions)

```
Server                              Client (projection)
──────                              ─────────────────
~/.claude.json git projects  ──┐
~/.synapse/projects.json     ──┼──► hello.cwds[]  ──► workspacePaths()
                               │
SessionSummary.cwd (immutable) ─┼──► state.sessions[] ──► filteredSessions(workspace)
                               │
create { opts.cwd }  ◄─────────┘    pendingCwd / draft (pre-create only)
register_project { path } ──► persists path, returns updated cwds
```

**Terminology:** **Workspace** is the canonical name in UI and docs. *Repo* and *Project* mean the same thing (a git working directory path). Use **Workspace** in user-facing copy.

**Mental model:** A **Workspace** is an absolute path to a folder (usually a git repo). A **Session** belongs to exactly one workspace (`session.cwd`, set at create). On **web**, the home screen lists workspaces with a session drawer. On **iOS**, SwiftUI owns the list + sheet; web is chat-only.

| Term (UI) | Protocol / code | Meaning |
|-----------|-----------------|---------|
| Workspace | `cwds[]`, `opts.cwd`, `session.cwd` | Absolute path to working directory |
| Workspaces (screen) | `state.view === "workspaces"` | List mode (CSS class unchanged) |
| Add workspace (`+`) | `op: register_project` | Register folder on server |
| New session | `op: create` | New Claude session in `pendingCwd` workspace |

**Server persistence:** manually added workspaces are stored in `~/.synapse/projects.json` and merged with Claude-discovered git repos on every `hello` / `refresh_cwds`. Wire op name `register_project` is historical.

---

## References (look, don't necessarily import)

| Source | Use for |
|--------|---------|
| [Apple HIG](https://developer.apple.com/design/human-interface-guidelines/) | Touch targets (44pt), margins, semantic colors, accessibility |
| [Konsta UI Kitchen Sink](https://konstaui.com/kitchen-sink/) | iOS component proportions — List, Navbar, Searchbar, Sheet Modal |
| [Konsta source](https://github.com/konstaui/konsta) | Pixel-level class names when translating patterns to vanilla CSS |
| [Framework7 iOS theme](https://framework7.io/) | List rows, pull-to-refresh, action sheet patterns (vanilla JS friendly) |

We **do not** ship a full UI framework. Borrow measurements and structure; keep the bundle zero-build and Cursor-specific where needed (composer, agent transcript).

---

## Principles

1. **One horizontal rhythm** — page content, topbar, search, and list rows share **16px** side padding unless noted.
2. **44pt touch, 32pt glyph** — interactive controls are 44×44px hit areas; visible circles/tiles are 32×32px.
3. **Semantic tokens only** — use `--ink`, `--accent`, `--btn-secondary`, not raw hex in new CSS.
4. **Explicit flex structure** — never stack unrelated controls as direct column children without a row wrapper (see Composer).
5. **Chat-only composer** — `#composer` lives inside `#chatView`; project list has no input bar.
6. **Light theme first** — `html.theme-light` + `color-scheme: light`; dark is out of scope until tokens are duplicated.

---

## Design tokens

Defined in `:root` in `app.css`. When adding styles, extend these — do not invent parallel variables.

### Color (iOS semantic mapping)

| Token | Value | HIG analogue |
|-------|-------|----------------|
| `--bg` / `--surface` | `#ffffff` | systemBackground |
| `--elevated` / `--control` | `#f2f2f7` | secondarySystemBackground |
| `--ink` | `#1c1c1e` | label |
| `--ink-2` | `rgba(60,60,67,.6)` | secondaryLabel |
| `--ink-3` | `rgba(60,60,67,.45)` | tertiaryLabel |
| `--btn-secondary` | `rgba(120,120,128,.12)` | systemFill |
| `--btn-secondary-hover` | `rgba(120,120,128,.18)` | systemFill (pressed) |
| `--accent` / `--accent-text` | `#007aff` | tintColor |
| `--accent-bg` | `rgba(0,122,255,.1)` | tint + fill |
| `--danger` | `#ff3b30` | systemRed |
| `--success` | `#34c759` | systemGreen |
| `--composer-bg` | `#f2f2f7` | grouped background |
| `--user-msg` | `#e8e8ed` | user bubble |
| `--primary` / `--primary-fg` | `#1c1c1e` / `#fff` | primary button (send) |

### Spacing scale

Use only these steps:

| Step | px | Typical use |
|------|-----|-------------|
| 2 | 2 | Icon button group gap (`.topbar-actions`) |
| 4 | 4 | Tight inline gaps |
| 6 | 6 | Search field icon gap |
| 8 | 8 | Composer internal gap, list section gap |
| 10 | 10 | Section padding |
| 12 | 12 | Row internal gap, composer padding |
| 16 | 16 | **Page horizontal margin** (topbar, title, lists, composer) |
| 20+ | 20+ | Empty state / marketing blocks only |

### Typography

| Role | Size | Weight | Where |
|------|------|--------|-------|
| Large title | 34px | 700 | `#pageTitle` |
| Nav title | 17px | 600 | `#chatTitle` |
| Body / list title | 17px | 400 | `.ws-label`, `.sess-title` |
| Subtitle | 15px | 400 | `.sess-sub`, empty state body |
| Composer input | 16px | 400 | `#input` (avoids iOS zoom-on-focus) |
| Caption / sheet label | 12–13px | 500–600 | `.sheet-item-label`, toast |
| Sheet title | 15px | 600 | `#sheetTitle` |

Font stack: `--font-ui` (SF Pro / system). Monospace: `--font-mono` for code only.

### Radius

| Token / class | px | Use |
|---------------|-----|-----|
| 50% | — | Circular icon buttons, send |
| 8px | — | Avatar / logo corner |
| 10px | — | Archived toggle, small cards |
| 12px | — | Suggestion chips, cards |
| 18px | — | Search field (height/2 pill) |
| 22px | — | Composer container |
| 14px top | — | Bottom sheet top corners |

### Layout constants

| Token | Value |
|-------|-------|
| `--topbar-h` | 44px |
| `--safe-top` / `--safe-bottom` | `env(safe-area-inset-*)` |
| `--kb` | Keyboard overlap (set from JS / native) |

---

## View modes

`body` class controls which regions are visible:

| Class | Shows | Hides |
|-------|-------|-------|
| `mode-workspaces` | `#topbar` (avatar + search + add), `#pageHead`, `#workspaceView` | `#chatView` |
| `mode-chat` | `#topbar` (back + title), `#chatView` + `#composer` | `#pageHead`, `#workspaceView` |

State in JS: `state.view` is `"workspaces"` | `"chat"` (internal name; user-facing label is **Projects**).

---

## Components

### Top bar (`#topbar`)

**Projects list mode**

```
[ avatar 32px ]                    [ search ] [ + add project ]
```

- `#newBtn` → `openAddProject()` → `register_project` on server (not a local-only list mutation).

**Chat mode** — show `#backBtn` only; center `#chatTitle`; hide avatar and `.topbar-actions`.

---

### Page title (`#pageHead`)

- Title: **Projects** (`#pageTitle`)
- `padding: 2px 16px 12px`

---

### Project tree (`#workspaceView`)

One row per project path; sessions nested when expanded. **No “All Repos” aggregate.**

**Project row** — `.ws-row.ws-tree-repo`

- Label: `basename(projectPath)`; optional `.ws-count` badge
- Click toggles `state.expandedProjects` (does not navigate away)
- Sorted by most recent session activity, then name
- On return from chat, only the active session’s project is auto-expanded (plus any with sessions on first load)

**Children** — `.ws-tree-children`

- `.tree-new-row` → `startNewDraft(projectPath)`
- `.sess-row` → open chat

**Empty catalog:** “No projects yet — Tap + to add a project folder”

**Konsta reference:** `List`, `ListItem`, chevron disclosure.

---

### Chat composer (`#composer`)

**Only visible in `mode-chat`.** Required DOM structure:

```html
<footer id="composer">
  <div id="composerDock">
    <div id="composerRow">
      <div class="composer-field">   <!-- row: do not skip this wrapper -->
        <button id="attachBtn" class="dock-btn" />
        <textarea id="input" />
      </div>
      <div id="composerControls">
        <button id="modelCtl" class="dock-btn dock-text" />
        <span class="spacer" />
        <button id="sendBtn" />
      </div>
    </div>
  </div>
  <div id="attachMenu" class="model-menu" />
</footer>
```

Layout:

```
┌─────────────────────────────────────┐
│  [+]  multiline textarea            │  ← .composer-field (flex row)
│  [ Auto ]              [ send ↑ ]   │  ← #composerControls
└─────────────────────────────────────┘
```

Rules:

- `#composerRow`: **column** flex; children are only `.composer-field` and `#composerControls`.
- **Never** put `#attachBtn` directly under `#composerRow` without `.composer-field` — column stretch will warp the button to full width.
- `.dock-btn`: fixed **32×32**; `flex: 0 0 32px`
- `#sendBtn`: 32px circle, `--primary` background; `.active` when text present; `.busy` shows stop icon
- Padding bottom: `calc(10px + var(--safe-bottom) + var(--kb))`

Placeholder: `Plan, ask, build…` (new) / `Follow up…` (existing session) — set in `updateChrome()`.

---

### Bottom sheet (`#bottomSheet`)

Used for thinking content, tool details, model picker (`.sheet-picker`).

- Max height 85vh (72vh for picker)
- Top handle: 36×4px, radius 2px
- Head: close + centered title
- Enter: `translateY(100%)` → `.show` → `translateY(0)`
- Mask: blur + 55% black

**Konsta reference:** `Sheet Modal`.

---

### Menus (`.model-menu`)

Popover anchored to composer or opened from `+` (add repo).

- `#attachMenu` — child of `#composer` (positioned above composer)
- `#localMenu` — body level (add project from projects list); call `stopPropagation` on trigger

---

### Empty state (`#empty`)

Centered in chat when no messages. Logo 56px, title “Let's build”, suggestion chips.

- Chips: full width, min-height 44px, radius 12px
- `pointer-events: none` on container; `auto` on `.suggestions button`

---

## Icons

- Top bar / dock: **16×16** SVG stroke icons
- Back: 18×18
- Folder / session list: 22×22
- Stroke width ~1.4–1.8; use `currentColor`

---

## Motion

| Element | Duration | Easing |
|---------|----------|--------|
| View switch | 200ms | ease-out |
| Sheet | 320ms | `cubic-bezier(.32,.72,0,1)` |
| Menu pop | 130ms | ease-out |
| Button press | — | `scale(0.94)` active |
| Icon btn | 150ms | background |

---

## Verification checklist

Before shipping UI changes:

1. Run `./scripts/verify-web.sh` from repo root.
2. Rebuild iOS sim: `./mobile/build-sim.sh` (web bundle is compiled into the binary).
3. Manual checks:
   - [ ] Projects list: no composer visible
   - [ ] `+` registers project via server; does not create session by itself
   - [ ] No duplicate session rows under “All Repos”
   - [ ] Tree expand/collapse; session opens chat
   - [ ] Composer: `+` button is **not** stretched horizontally
   - [ ] Keyboard: composer sits above keyboard; no iOS autofill bar regression
   - [ ] Topbar aligns with “Workspaces” title at 16px

---

## File map

| File | Responsibility |
|------|----------------|
| `index.html` | Structure, ARIA, component skeleton |
| `app.css` | Tokens + all presentation |
| `app.js` | Behavior, `renderWorkspaceTree()`, view mode |
| `verify-ui.mjs` | Playwright regression tests |
| `DESIGN.md` | This document |

**Future (optional):** split `app.css` → `tokens.css` + `components.css`; keep a single import in HTML until a build step exists.

---

## Anti-patterns

| Don't | Do instead |
|-------|------------|
| Raw hex in new rules | Add/use `--token` |
| 12px page margins | 16px to match title |
| 36px touch-only buttons without 44pt hit | `.iconbtn` pattern |
| Flat flex column with mixed row controls | Wrapper rows (`.composer-field`) |
| Composer on projects list | Chat-only `#composer` |
| `newBtn` → new session | `newBtn` → `register_project`; new session via `.tree-new-row` |
| Client-only `cwds.push()` | Server `register_project` + `cwds` broadcast |
| “All Repos” duplicate tree | One session list per project folder only |
