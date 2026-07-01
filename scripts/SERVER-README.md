# Synapse Server

Remote control bridge for **Claude Code CLI** on your computer.  
Your phone connects through the Synapse cloud relay — no port forwarding or manual URLs.

## Quick start

### One-line install (macOS / Linux)

**国内推荐（走 relay 镜像，无需访问 githubusercontent）：**

```sh
curl -fsSL https://zx0623.duckdns.org/install.sh | bash
```

**GitHub Releases 直连：**

```sh
curl -fsSL https://github.com/zhaoxini/synapse/releases/latest/download/install.sh | bash
```

### Homebrew (macOS / Linuxbrew)

```sh
brew tap zhaoxini/synapse https://github.com/zhaoxini/synapse
brew install synapse-server
```

### Manual download

Download a release archive from [GitHub Releases](https://github.com/zhaoxini/synapse/releases), then:

```sh
tar xzf synapse-*-x86_64-unknown-linux-gnu.tar.gz
cd synapse-*-x86_64-unknown-linux-gnu
sudo ./install.sh
```

### Run

```sh
synapse-server
```

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
# 国内推荐
curl -fsSL https://zx0623.duckdns.org/install-relay.sh | sudo bash

# GitHub 直连
curl -fsSL https://github.com/zhaoxini/synapse/releases/latest/download/install-relay.sh | sudo bash
```

Default relay domain: `wss://zx0623.duckdns.org` (baked into release `synapse-server` builds).

Manual setup:

```sh
./bin/synapse-relay --port 443 --tls-cert fullchain.pem --tls-key privkey.pem --public-host zx0623.duckdns.org
```

Set the repository variable `SYNAPSE_RELAY=wss://relay.example.com` when building release binaries to override the default relay URL.
