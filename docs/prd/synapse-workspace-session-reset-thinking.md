# Synapse workspace/session/reset + reasoning PRD

## Goal

Make workspace import, session import, reset, and model reasoning controls explicit.

## Requirements

- Workspace import only registers a real repo path.
- Existing Claude Code sessions are never auto-attached on server startup or refresh.
- Repo screen `+` owns session creation/import.
- Reset clears Synapse-owned data and Synapse session index, but not repos or Claude Code transcripts.
- Model effort/thinking capability data comes from Synapse server, not hardcoded UI guesses.

## Data ownership

- Server owns durable workspace/session/model metadata.
- Client owns transient UI state and local pairing credentials.
- Claude Code remains source of truth for original transcripts; Synapse only attaches/imports selected sessions.

## Server contract

### `hello`

`models[]` includes capability metadata:

```json
{
  "id": "opus",
  "label": "Opus",
  "effortLevels": ["low", "medium", "high", "xhigh", "max"],
  "thinking": { "supported": true, "canDisable": true, "adaptive": true }
}
```

### Workspace import

```json
{ "op": "register_project", "path": "/repo/path" }
```

Response:

```json
{ "type": "cwds", "cwds": [], "registeredProjects": ["/repo/path"] }
```

No session attach, no session create.

### Session import

```json
{ "op": "list_importable_sessions", "cwd": "/repo/path" }
{ "op": "import_sessions", "cwd": "/repo/path", "sessionIds": ["..."] }
```

Only attachable sessions from the same cwd are listed/imported. Stopped/completed sessions stay hidden; active background sessions are importable when they belong to the current workspace.

### Reset

```json
{ "op": "reset_data", "confirm": "RESET" }
```

Clears:

- `~/.synapse/projects.json`
- `~/.synapse/session_meta.json`
- `~/.synapse/models.json`
- `~/.synapse/config.json`
- `~/.synapse/pairing-code`
- in-memory Synapse sessions and UI state
- browser `synapse_creds`

Does not clear:

- repo files
- `.git`
- `~/.claude` transcripts/session store
- Claude Code auth
- cloud account

## Reasoning capabilities

Server infers capabilities from model ID/alias plus optional `~/.synapse/models.json` custom capabilities.

| Model | Effort |
| --- | --- |
| Fable 5 | `low`, `medium`, `high`, `xhigh`, `max` |
| Sonnet 5 / Opus 4.8 / Opus 4.7 | `low`, `medium`, `high`, `xhigh`, `max` |
| Opus 4.6 / Sonnet 4.6 | `low`, `medium`, `high`, `max` |
| Haiku / unknown | none unless custom capabilities declare support |

Thinking off maps to `MAX_THINKING_TOKENS=0`. Fable cannot disable thinking.

## Acceptance

- Add workspace does not change session count.
- Server startup/refresh does not auto-import Claude Code sessions.
- Repo `+` opens Add Session; import works only for current repo.
- Reset returns to pairing overlay and clears sessions/workspaces.
- Model sheet shows Reasoning from server-provided capabilities.
- `--effort` and `MAX_THINKING_TOKENS=0` reach `claude -p` on next turn.
