use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

pub struct Db {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub email: String,
    pub name: String,
    pub email_verified: bool,
}

#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub device_token: String,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path).context("open sqlite db")?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                email TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                name TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS sessions (
                token TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id),
                expires_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS devices (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id),
                name TEXT NOT NULL,
                device_token TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS pairing_codes (
                code TEXT PRIMARY KEY,
                device_id TEXT NOT NULL REFERENCES devices(id),
                expires_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS connect_tokens (
                token TEXT PRIMARY KEY,
                device_id TEXT NOT NULL REFERENCES devices(id),
                user_id TEXT NOT NULL REFERENCES users(id),
                expires_at INTEGER NOT NULL
            );
            PRAGMA user_version = 1;
            ",
        )?;
        self.migrate_v2()?;
        Ok(())
    }

    fn migrate_v2(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let ver: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if ver >= 2 {
            return Ok(());
        }
        let _ = conn.execute(
            "ALTER TABLE users ADD COLUMN email_verified INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute("ALTER TABLE users ADD COLUMN oauth_provider TEXT", []);
        let _ = conn.execute("ALTER TABLE users ADD COLUMN oauth_sub TEXT", []);
        conn.execute_batch(
            "
            UPDATE users SET email_verified = 1;
            CREATE TABLE IF NOT EXISTS email_codes (
                email TEXT NOT NULL,
                purpose TEXT NOT NULL,
                code TEXT NOT NULL,
                expires_at INTEGER NOT NULL,
                PRIMARY KEY (email, purpose)
            );
            CREATE TABLE IF NOT EXISTS oauth_states (
                state TEXT PRIMARY KEY,
                expires_at INTEGER NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_users_oauth ON users(oauth_provider, oauth_sub)
                WHERE oauth_provider IS NOT NULL AND oauth_sub IS NOT NULL;
            PRAGMA user_version = 2;
            ",
        )?;
        Ok(())
    }

    pub fn create_user(
        &self,
        id: &str,
        email: &str,
        password_hash: &str,
        name: &str,
        email_verified: bool,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        self.conn.lock().unwrap().execute(
            "INSERT INTO users (id, email, password_hash, name, created_at, email_verified) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, email, password_hash, name, now, email_verified as i32],
        )?;
        Ok(())
    }

    pub fn create_oauth_user(
        &self,
        id: &str,
        email: &str,
        name: &str,
        provider: &str,
        sub: &str,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        self.conn.lock().unwrap().execute(
            "INSERT INTO users (id, email, password_hash, name, created_at, email_verified, oauth_provider, oauth_sub)
             VALUES (?1, ?2, '', ?3, ?4, 1, ?5, ?6)",
            params![id, email, name, now, provider, sub],
        )?;
        Ok(())
    }

    pub fn user_by_oauth(&self, provider: &str, sub: &str) -> Result<Option<User>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, email, name, email_verified FROM users WHERE oauth_provider = ?1 AND oauth_sub = ?2",
        )?;
        let mut rows = stmt.query(params![provider, sub])?;
        if let Some(row) = rows.next()? {
            Ok(Some(User {
                id: row.get(0)?,
                email: row.get(1)?,
                name: row.get(2)?,
                email_verified: row.get::<_, i32>(3)? != 0,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn set_email_verified(&self, user_id: &str, verified: bool) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE users SET email_verified = ?1 WHERE id = ?2",
            params![verified as i32, user_id],
        )?;
        Ok(())
    }

    pub fn update_password(&self, user_id: &str, password_hash: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE users SET password_hash = ?1 WHERE id = ?2",
            params![password_hash, user_id],
        )?;
        Ok(())
    }

    pub fn link_oauth(&self, user_id: &str, provider: &str, sub: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE users SET oauth_provider = ?1, oauth_sub = ?2, email_verified = 1 WHERE id = ?3",
            params![provider, sub, user_id],
        )?;
        Ok(())
    }

    pub fn store_email_code(
        &self,
        email: &str,
        purpose: &str,
        code: &str,
        expires_at: i64,
    ) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT OR REPLACE INTO email_codes (email, purpose, code, expires_at) VALUES (?1, ?2, ?3, ?4)",
            params![email, purpose, code, expires_at],
        )?;
        Ok(())
    }

    pub fn verify_email_code(&self, email: &str, purpose: &str, code: &str) -> Result<bool> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT 1 FROM email_codes WHERE email = ?1 AND purpose = ?2 AND code = ?3 AND expires_at > ?4",
        )?;
        let ok = stmt
            .query(params![email, purpose, code, now])?
            .next()?
            .is_some();
        if ok {
            conn.execute(
                "DELETE FROM email_codes WHERE email = ?1 AND purpose = ?2",
                params![email, purpose],
            )?;
        }
        Ok(ok)
    }

    pub fn store_oauth_state(&self, state: &str, expires_at: i64) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT OR REPLACE INTO oauth_states (state, expires_at) VALUES (?1, ?2)",
            params![state, expires_at],
        )?;
        Ok(())
    }

    pub fn consume_oauth_state(&self, state: &str) -> Result<bool> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let ok = conn
            .prepare("SELECT 1 FROM oauth_states WHERE state = ?1 AND expires_at > ?2")?
            .query(params![state, now])?
            .next()?
            .is_some();
        if ok {
            conn.execute("DELETE FROM oauth_states WHERE state = ?1", params![state])?;
        }
        Ok(ok)
    }

    pub fn delete_session(&self, token: &str) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM sessions WHERE token = ?1", params![token])?;
        Ok(())
    }

    pub fn user_by_email(&self, email: &str) -> Result<Option<(User, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, email, name, email_verified, password_hash FROM users WHERE email = ?1")?;
        let mut rows = stmt.query(params![email])?;
        if let Some(row) = rows.next()? {
            Ok(Some((
                User {
                    id: row.get(0)?,
                    email: row.get(1)?,
                    name: row.get(2)?,
                    email_verified: row.get::<_, i32>(3)? != 0,
                },
                row.get(4)?,
            )))
        } else {
            Ok(None)
        }
    }

    pub fn user_by_id(&self, id: &str) -> Result<Option<User>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, email, name, email_verified FROM users WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(User {
                id: row.get(0)?,
                email: row.get(1)?,
                name: row.get(2)?,
                email_verified: row.get::<_, i32>(3)? != 0,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn create_session(&self, token: &str, user_id: &str, expires_at: i64) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO sessions (token, user_id, expires_at) VALUES (?1, ?2, ?3)",
            params![token, user_id, expires_at],
        )?;
        Ok(())
    }

    pub fn session_user_id(&self, token: &str) -> Result<Option<String>> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT user_id FROM sessions WHERE token = ?1 AND expires_at > ?2")?;
        let mut rows = stmt.query(params![token, now])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn create_device(
        &self,
        id: &str,
        user_id: &str,
        name: &str,
        device_token: &str,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        self.conn.lock().unwrap().execute(
            "INSERT INTO devices (id, user_id, name, device_token, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, user_id, name, device_token, now],
        )?;
        Ok(())
    }

    pub fn devices_for_user(&self, user_id: &str) -> Result<Vec<Device>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, user_id, name, device_token FROM devices WHERE user_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok(Device {
                id: row.get(0)?,
                user_id: row.get(1)?,
                name: row.get(2)?,
                device_token: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn device_by_id(&self, id: &str) -> Result<Option<Device>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, user_id, name, device_token FROM devices WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Device {
                id: row.get(0)?,
                user_id: row.get(1)?,
                name: row.get(2)?,
                device_token: row.get(3)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn verify_device_token(&self, device_id: &str, token: &str) -> Result<bool> {
        Ok(self
            .device_by_id(device_id)?
            .map(|d| d.device_token == token)
            .unwrap_or(false))
    }

    pub fn device_owned_by(&self, device_id: &str, user_id: &str) -> Result<bool> {
        Ok(self
            .device_by_id(device_id)?
            .map(|d| d.user_id == user_id)
            .unwrap_or(false))
    }

    pub fn create_pairing_code(&self, code: &str, device_id: &str, expires_at: i64) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT OR REPLACE INTO pairing_codes (code, device_id, expires_at) VALUES (?1, ?2, ?3)",
            params![code, device_id, expires_at],
        )?;
        Ok(())
    }

    pub fn pairing_code_device(&self, code: &str) -> Result<Option<String>> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT device_id FROM pairing_codes WHERE code = ?1 AND expires_at > ?2")?;
        let mut rows = stmt.query(params![code, now])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn delete_pairing_code(&self, code: &str) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM pairing_codes WHERE code = ?1", params![code])?;
        Ok(())
    }

    pub fn create_connect_token(
        &self,
        token: &str,
        device_id: &str,
        user_id: &str,
        expires_at: i64,
    ) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO connect_tokens (token, device_id, user_id, expires_at) VALUES (?1, ?2, ?3, ?4)",
            params![token, device_id, user_id, expires_at],
        )?;
        Ok(())
    }

    pub fn verify_connect_token(&self, token: &str, device_id: &str) -> Result<bool> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT 1 FROM connect_tokens WHERE token = ?1 AND device_id = ?2 AND expires_at > ?3",
        )?;
        let mut rows = stmt.query(params![token, device_id, now])?;
        Ok(rows.next()?.is_some())
    }
}
