## Synapse v0.2.4 — Background server start

- `synapse-server` now starts in the **background** by default (wrapper)
- Prints **listen port**, **pairing code**, and **web URL** on start
- `synapse-server stop` to stop; logs in `~/.synapse/server.log`
- `SYNAPSE_AUTO_START=1` on install to start immediately

---

## Synapse v0.2.0 — Account-based remote access

### What's included

Each archive contains:

- `bin/synapse-server` — run on your computer (one command: `./bin/synapse-server`)
- `bin/synapse-relay` — optional, deploy on your VPS
- `install.sh` — install to `/usr/local/bin`
- `README.md` — usage

### Platforms

| Archive | OS / CPU |
|---------|----------|
| `synapse-*-x86_64-unknown-linux-gnu.tar.gz` | Linux x64 |
| `synapse-*-aarch64-unknown-linux-gnu.tar.gz` | Linux ARM64 |
| `synapse-*-x86_64-apple-darwin.tar.gz` | macOS Intel |
| `synapse-*-aarch64-apple-darwin.tar.gz` | macOS Apple Silicon |
| `synapse-*-x86_64-pc-windows-msvc.zip` | Windows x64 |

First run asks for **email** and **password** (creates an account if needed).  
Then it prints a **6-digit pairing code** — enter it in the Synapse mobile app, or sign in with the same account and tap your computer.

## Quick start

```sh
# One-line install
curl -fsSL https://github.com/zhaoxini/synapse/releases/latest/download/install.sh | bash

# Or Homebrew
brew tap zhaoxini/synapse https://github.com/zhaoxini/synapse
brew install synapse-server

# Default relay: wss://zx0623.duckdns.org (no manual config needed)

# Then run
synapse-server
```
