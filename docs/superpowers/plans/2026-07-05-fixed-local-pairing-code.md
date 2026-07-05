# Fixed Local Pairing Code Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Synapse use a fixed local 6-digit pairing code that is generated on login, reused on restart, and registered with the relay for `code -> device_id` lookup.

**Architecture:** Keep the code source of truth on the local machine in `~/.synapse/pairing-code`. The server registers that caller-provided code with the relay on startup and periodically retries the same code; the relay stores the mapping and issues normal connect tokens when clients exchange the code. Login becomes interactive when arguments are omitted, while preserving flags for automation.

**Tech Stack:** Rust, Clap, Axum, Reqwest, SQLite via rusqlite, existing Synapse server/relay crates.

## Global Constraints

- Default relay remains baked via `SYNAPSE_DEFAULT_RELAY` and `account::default_relay_url()`.
- No new dependencies unless already present in workspace.
- Code is 6 ASCII digits.
- Existing LAN/local web pairing path must keep working.
- Do not change mobile UI unless API shape requires it.

---

## File Structure

- `crates/server/src/account.rs`: local config, interactive login helpers, local pairing code generation/reuse, relay pairing registration client.
- `crates/server/src/main.rs`: CLI option shape and run/login/pairing-code flow.
- `crates/relay/src/api.rs`: accept caller-provided pairing code and keep exchange semantics.
- `crates/relay/src/db.rs`: make pairing code creation idempotent for fixed local codes.

---

### Task 1: Local Fixed Pairing Code Helpers

**Files:**
- Modify: `crates/server/src/account.rs`

**Interfaces:**
- Produces: `pub fn load_or_create_pairing_code(reset: bool) -> Result<String>`
- Produces: `fn gen_pairing_code() -> String`
- Consumes: existing `pairing_code_path()`, `save_pairing_code()`, `load_pairing_code()`

- [ ] **Step 1: Add failing tests**

Add inside `#[cfg(test)] mod tests` in `crates/server/src/account.rs`:

```rust
#[test]
fn fixed_pairing_code_reuses_saved_value() {
    let dir = tempfile_home();
    let _guard = HomeGuard::set(&dir);

    save_pairing_code("123456").unwrap();
    let code = load_or_create_pairing_code(false).unwrap();

    assert_eq!(code, "123456");
}

#[test]
fn fixed_pairing_code_reset_replaces_saved_value() {
    let dir = tempfile_home();
    let _guard = HomeGuard::set(&dir);

    save_pairing_code("123456").unwrap();
    let code = load_or_create_pairing_code(true).unwrap();

    assert_ne!(code, "123456");
    assert_eq!(code.len(), 6);
    assert!(code.chars().all(|c| c.is_ascii_digit()));
    assert_eq!(load_pairing_code().unwrap(), code);
}

#[test]
fn fixed_pairing_code_repairs_invalid_saved_value() {
    let dir = tempfile_home();
    let _guard = HomeGuard::set(&dir);

    let path = pairing_code_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "bad\n").unwrap();

    let code = load_or_create_pairing_code(false).unwrap();

    assert_eq!(code.len(), 6);
    assert!(code.chars().all(|c| c.is_ascii_digit()));
    assert_eq!(load_pairing_code().unwrap(), code);
}
```

If `account.rs` has no test helpers, add minimal test-only helpers in the same module:

```rust
#[cfg(test)]
fn tempfile_home() -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("synapse-account-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(test)]
struct HomeGuard {
    old: Option<std::ffi::OsString>,
}

#[cfg(test)]
impl HomeGuard {
    fn set(path: &std::path::Path) -> Self {
        let old = std::env::var_os("HOME");
        std::env::set_var("HOME", path);
        Self { old }
    }
}

#[cfg(test)]
impl Drop for HomeGuard {
    fn drop(&mut self) {
        if let Some(old) = self.old.take() {
            std::env::set_var("HOME", old);
        } else {
            std::env::remove_var("HOME");
        }
    }
}
```

- [ ] **Step 2: Run tests to verify fail**

Run:

```bash
cargo test -p synapse-server fixed_pairing_code
```

Expected: FAIL with unresolved `load_or_create_pairing_code`.

- [ ] **Step 3: Implement fixed code helper**

Add to `crates/server/src/account.rs` near `load_pairing_code()`:

```rust
fn gen_pairing_code() -> String {
    use rand::Rng;
    format!("{:06}", rand::thread_rng().gen_range(0..1_000_000))
}

pub fn load_or_create_pairing_code(reset: bool) -> Result<String> {
    if !reset {
        if let Some(code) = load_pairing_code() {
            return Ok(code);
        }
    }
    let code = gen_pairing_code();
    save_pairing_code(&code)?;
    Ok(code)
}
```

- [ ] **Step 4: Run tests to verify pass**

Run:

```bash
cargo test -p synapse-server fixed_pairing_code
```

Expected: PASS.

---

### Task 2: Relay Accepts Caller-Provided Pairing Code

**Files:**
- Modify: `crates/relay/src/api.rs`
- Modify: `crates/relay/src/db.rs`

**Interfaces:**
- Consumes: `POST /api/v1/pairing-codes` with Device auth.
- Produces: optional JSON request body `{ "code": "123456" }`.
- Produces: idempotent `Db::create_pairing_code(&code, &device_id, expires_at)` that overwrites same code/device mapping safely.

- [ ] **Step 1: Add failing relay API unit test**

Add near existing tests in `crates/relay/src/api.rs` if present, otherwise create `#[cfg(test)] mod tests` at bottom:

```rust
#[test]
fn pairing_code_body_accepts_six_digits() {
    let body: CreatePairingCodeBody = serde_json::from_str(r#"{"code":"123456"}"#).unwrap();
    assert_eq!(body.code.as_deref(), Some("123456"));
}
```

- [ ] **Step 2: Run test to verify fail**

Run:

```bash
cargo test -p synapse-relay pairing_code_body_accepts_six_digits
```

Expected: FAIL with unresolved `CreatePairingCodeBody`.

- [ ] **Step 3: Add request body and validation**

In `crates/relay/src/api.rs`, replace `new_pairing_code` import use only if still needed. Add:

```rust
#[derive(Deserialize)]
struct CreatePairingCodeBody {
    #[serde(default)]
    code: Option<String>,
}

fn clean_pairing_code(input: &str) -> Option<String> {
    let code: String = input.chars().filter(|c| c.is_ascii_digit()).collect();
    if code.len() == 6 { Some(code) } else { None }
}
```

Change handler signature:

```rust
async fn create_pairing_code(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<CreatePairingCodeBody>>,
) -> impl IntoResponse {
```

Inside handler, replace new/extend branching with:

```rust
let expires = chrono::Utc::now().timestamp() + PAIRING_CODE_SECS;
let requested = body
    .and_then(|Json(b)| b.code)
    .and_then(|c| clean_pairing_code(&c));
let code = match requested {
    Some(c) => c,
    None => match s.db.pairing_code_for_device(&device_id) {
        Ok(Some((existing, _))) => existing,
        Ok(None) => new_pairing_code(),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    },
};
if let Err(e) = s.db.create_pairing_code(&code, &device_id, expires) {
    return api_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
}
Json(PairingCodeResp { code, expires_in: PAIRING_CODE_SECS }).into_response()
```

- [ ] **Step 4: Make DB insert idempotent by code and device**

In `crates/relay/src/db.rs`, replace `create_pairing_code` body with:

```rust
pub fn create_pairing_code(&self, code: &str, device_id: &str, expires_at: i64) -> Result<()> {
    let conn = self.conn.lock().unwrap();
    conn.execute(
        "DELETE FROM pairing_codes WHERE device_id = ?1 OR code = ?2",
        params![device_id, code],
    )?;
    conn.execute(
        "INSERT INTO pairing_codes (code, device_id, expires_at) VALUES (?1, ?2, ?3)",
        params![code, device_id, expires_at],
    )?;
    Ok(())
}
```

- [ ] **Step 5: Run relay tests**

Run:

```bash
cargo test -p synapse-relay pairing_code_body_accepts_six_digits
cargo test -p synapse-relay
```

Expected: PASS.

---

### Task 3: Server Registers Fixed Code With Relay

**Files:**
- Modify: `crates/server/src/account.rs`
- Modify: `crates/server/src/main.rs`

**Interfaces:**
- Consumes: `load_or_create_pairing_code(reset: bool) -> Result<String>` from Task 1.
- Produces: `pub async fn register_pairing_code(cfg: &Config, code: &str) -> Result<PairingCodeResp>`.
- Produces: `pub fn spawn_pairing_registration(cfg: Config, code: String)`.

- [ ] **Step 1: Add failing test for request body serialization**

In `crates/server/src/account.rs`, add:

```rust
#[test]
fn pairing_code_request_serializes_code() {
    let body = PairingCodeBody { code: "123456" };
    let json = serde_json::to_value(body).unwrap();
    assert_eq!(json["code"], "123456");
}
```

- [ ] **Step 2: Run test to verify fail**

Run:

```bash
cargo test -p synapse-server pairing_code_request_serializes_code
```

Expected: FAIL with unresolved `PairingCodeBody`.

- [ ] **Step 3: Implement relay registration client**

In `crates/server/src/account.rs`, add:

```rust
#[derive(Serialize)]
struct PairingCodeBody<'a> {
    code: &'a str,
}

pub async fn register_pairing_code(cfg: &Config, code: &str) -> Result<PairingCodeResp> {
    let client = reqwest::Client::new();
    let resp: PairingCodeResp = client
        .post(format!("{}/api/v1/pairing-codes", cfg.relay_api))
        .header(
            "Authorization",
            format!("Device {}:{}", cfg.device_id, cfg.device_token),
        )
        .json(&PairingCodeBody { code })
        .send()
        .await
        .context("pairing code registration request")?
        .error_for_status()
        .context("pairing code registration failed")?
        .json()
        .await
        .context("pairing code registration response")?;
    save_pairing_code(&resp.code)?;
    Ok(resp)
}
```

Keep `create_pairing_code` as wrapper for compatibility:

```rust
pub async fn create_pairing_code(cfg: &Config) -> Result<PairingCodeResp> {
    let code = load_or_create_pairing_code(false)?;
    register_pairing_code(cfg, &code).await
}
```

Replace refresh loop with same-code registration:

```rust
pub fn spawn_pairing_registration(cfg: Config, code: String) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(240));
        interval.tick().await;
        loop {
            interval.tick().await;
            match register_pairing_code(&cfg, &code).await {
                Ok(resp) => tracing::debug!(code = %resp.code, "pairing code registered"),
                Err(e) => tracing::warn!("pairing code registration failed: {e}"),
            }
        }
    });
}
```

- [ ] **Step 4: Update run flow**

In `crates/server/src/main.rs`, replace account-mode pairing block with:

```rust
if let Some(cfg) = &saved {
    println!("  Signed in as:   {}", cfg.user_email);
    println!("  This machine:   {}", cfg.device_name);
    let code = account::load_or_create_pairing_code(false)?;
    match account::register_pairing_code(cfg, &code).await {
        Ok(resp) => {
            println!("\n  ┌─────────────────────────────────────┐");
            println!("  │  Pairing code:  {:>6}               │", resp.code);
            println!("  └─────────────────────────────────────┘");
            println!("\n  Web:  http://127.0.0.1:8000/?code={}", resp.code);
            println!("  (code stays stable until you log in again)\n");
        }
        Err(e) => {
            println!("\n  Pairing code:  {code}");
            println!("  Web:  http://127.0.0.1:8000/?code={code}");
            tracing::warn!("pairing code registration failed: {e}");
        }
    }
    account::spawn_pairing_registration(cfg.clone(), code);
}
```

- [ ] **Step 5: Run server tests**

Run:

```bash
cargo test -p synapse-server pairing_code_request_serializes_code
cargo test -p synapse-server fixed_pairing_code
```

Expected: PASS.

---

### Task 4: Interactive Login Defaults

**Files:**
- Modify: `crates/server/src/main.rs`
- Modify: `crates/server/src/account.rs`

**Interfaces:**
- Consumes: `account::default_relay_url() -> Option<String>`.
- Consumes: `account::read_line(prompt) -> Result<String>` and `account::read_password(prompt) -> Result<String>`.
- Produces: `synapse-server login` with zero args prompts for email/password and uses default relay.

- [ ] **Step 1: Update Clap shape**

In `crates/server/src/main.rs`, change login fields to optional:

```rust
Login {
    #[arg(long)]
    relay: Option<String>,
    #[arg(long)]
    email: Option<String>,
    #[arg(long)]
    password: Option<String>,
    #[arg(long)]
    device_name: Option<String>,
},
```

- [ ] **Step 2: Add helper for login prompt**

In `crates/server/src/account.rs`, add:

```rust
pub fn resolve_relay_arg(relay: Option<String>) -> Result<String> {
    relay
        .filter(|s| !s.trim().is_empty())
        .or_else(default_relay_url)
        .context("relay URL required")
}

pub fn resolve_email_arg(email: Option<String>) -> Result<String> {
    let email = match email {
        Some(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => read_line("Email: ")?,
    };
    if email.is_empty() || !email.contains('@') {
        bail!("valid email required");
    }
    Ok(email)
}

pub fn resolve_password_arg(password: Option<String>) -> Result<String> {
    match password {
        Some(v) if !v.is_empty() => Ok(v),
        _ => read_password("Password: "),
    }
}
```

- [ ] **Step 3: Update login command flow**

In `crates/server/src/main.rs`, replace `Some(Commands::Login { ... })` arm body with:

```rust
let relay = account::resolve_relay_arg(relay)?;
let email = account::resolve_email_arg(email)?;
let password = account::resolve_password_arg(password)?;
let device_name = device_name.unwrap_or_else(account::default_device_name);
let cfg = account::login_account(&relay, &email, &password, &device_name).await?;
cfg.save()?;
let code = account::load_or_create_pairing_code(true)?;
let _ = account::register_pairing_code(&cfg, &code).await;
println!("\n  Logged in and device registered.\n");
println!("  Email:       {}", cfg.user_email);
println!("  Device:      {} ({})", cfg.device_name, cfg.device_id);
println!("  Pairing:     {}", code);
println!("  Config:      {}", account::Config::path().display());
println!("\n  Run: synapse-server\n");
Ok(())
```

- [ ] **Step 4: Update interactive setup to generate/reset code**

In `interactive_setup()`, after `cfg.save()?`, add:

```rust
let _ = load_or_create_pairing_code(true)?;
```

Do this in both login and register success branches.

- [ ] **Step 5: Check CLI help**

Run:

```bash
cargo run -p synapse-server -- login --help
```

Expected: help shows optional `--relay`, `--email`, `--password`; no “required arguments were not provided” when invoked without args in a terminal.

---

### Task 5: Pairing-Code Command Uses Fixed Local Code

**Files:**
- Modify: `crates/server/src/main.rs`

**Interfaces:**
- Consumes: `load_or_create_pairing_code(false)`.
- Consumes: `register_pairing_code(cfg, code)`.
- Produces: `synapse-server pairing-code` prints fixed code even if relay registration fails.

- [ ] **Step 1: Replace pairing-code command flow**

In `crates/server/src/main.rs`, replace `Some(Commands::PairingCode)` arm with:

```rust
let cfg = account::Config::load()?.context("not signed in — run synapse-server login")?;
let code = account::load_or_create_pairing_code(false)?;
match account::register_pairing_code(&cfg, &code).await {
    Ok(resp) => println!("\n  Pairing code:  {}\n", resp.code),
    Err(e) => {
        tracing::warn!("pairing code registration failed: {e}");
        println!("\n  Pairing code:  {}\n", code);
    }
}
println!("  Stable until you log in again.");
println!("  Web: http://127.0.0.1:8000/?code={code}\n");
Ok(())
```

- [ ] **Step 2: Run command against current config**

Run:

```bash
target/release/synapse-server pairing-code || cargo run -p synapse-server -- pairing-code
```

Expected: prints a 6-digit code even if relay returns 401.

---

### Task 6: Full Verification

**Files:**
- No code changes expected.

**Interfaces:**
- Verifies tasks 1-5 and existing PRD/UI work still pass.

- [ ] **Step 1: Run Rust tests**

Run:

```bash
cargo test --workspace
```

Expected: all tests pass.

- [ ] **Step 2: Run web UI verification**

Run:

```bash
./scripts/verify-web.sh
```

Expected: `=== OK — web UI verified ===`.

- [ ] **Step 3: Start server detached and inspect output**

Run:

```bash
kill $(cat /tmp/synapse-server-local.pid) 2>/dev/null || true
nohup env SYNAPSE_WEB_DIR=$PWD/crates/app/web $PWD/target/release/synapse-server --cwd $PWD >/tmp/synapse-server-local.log 2>&1 & echo $! >/tmp/synapse-server-local.pid
sleep 1
sed -n '1,80p' /tmp/synapse-server-local.log
```

Expected: output includes `Pairing code:  <six digits>` and `Web:  http://127.0.0.1:8000/?code=<same code>`.

- [ ] **Step 4: Smoke web and API ports**

Run:

```bash
curl -sS -m 2 -o /tmp/synapse-web-check.html -w 'web %{http_code} %{size_download}\n' http://127.0.0.1:8000/
curl -sS -m 2 -o /tmp/synapse-api-check.txt -w 'api %{http_code} %{size_download}\n' http://127.0.0.1:4173/ || true
```

Expected: `web 200 ...`; API root may be `400` because WS endpoint expects upgrade/query.

- [ ] **Step 5: Build iOS simulator if simulator booted**

Run:

```bash
./mobile/build-sim.sh
```

Expected: app builds, installs, launches. If no simulator booted, report exact `error: no booted simulator` and do not treat as code failure.

---

## Self-Review

- Spec coverage: login optional/interactively prompted in Task 4; fixed local code in Task 1; relay registration mapping in Tasks 2-3; stable pairing-code command in Task 5; verification in Task 6.
- Placeholder scan: no TBD/TODO/fill-in steps.
- Type consistency: `load_or_create_pairing_code`, `register_pairing_code`, `spawn_pairing_registration`, and `CreatePairingCodeBody` names match across tasks.
