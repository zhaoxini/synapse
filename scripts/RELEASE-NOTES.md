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

### User flow

1. Download the archive for your platform
2. Run `./bin/synapse-server`
3. Sign in with email + password
4. Open Synapse app on phone → same account → tap your computer

### Install script

```sh
tar xzf synapse-*-x86_64-unknown-linux-gnu.tar.gz
cd synapse-*-x86_64-unknown-linux-gnu
sudo ./install.sh
synapse-server
```
