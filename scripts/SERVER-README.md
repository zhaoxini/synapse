# Synapse Server

Remote control bridge for **Claude Code CLI** on your computer.  
Your phone connects through the Synapse cloud relay — no port forwarding or manual URLs.

## Quick start

```sh
# macOS / Linux
chmod +x bin/synapse-server
./bin/synapse-server
```

First run asks for **email** and **password** (creates an account if needed).  
Then it prints a **6-digit pairing code** — enter it in the Synapse mobile app, or sign in with the same account and tap your computer.

## Requirements

- [Claude Code CLI](https://docs.anthropic.com/en/docs/claude-code) (`claude`) on your PATH
- Outbound internet (for relay uplink)

## Commands

| Command | Description |
|---------|-------------|
| `synapse-server` | Start (default) |
| `synapse-server status` | Show signed-in account / device |
| `synapse-server pairing-code` | Print a new 6-digit code |
| `synapse-server logout` | Remove local credentials |

Advanced flags (`--port`, `--cwd`, `--tls`, …) are for developers; normal users don't need them.

## Config

Saved to `~/.synapse/config.json` after first sign-in.

## Relay (for operators)

This package also includes `bin/synapse-relay` — deploy on your VPS:

```sh
./bin/synapse-relay --port 443 --tls-cert fullchain.pem --tls-key privkey.pem --public-host relay.example.com
```

Set the repository variable `SYNAPSE_RELAY=wss://relay.example.com` when building release binaries so users never configure the relay URL themselves.
