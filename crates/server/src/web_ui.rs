//! Local web chat UI on :8000 (account / relay pairing mode).

use axum::Router;
use std::path::{Path, PathBuf};
use tower_http::services::{ServeDir, ServeFile};
use tracing::warn;

const WEB_PORT: u16 = 8000;

pub fn spawn() {
    let Some(dir) = resolve_web_dir() else {
        warn!(
            "web UI files not found — open http://127.0.0.1:{WEB_PORT}/ will not work; \
             reinstall synapse or copy crates/app/web to ~/.synapse/web"
        );
        return;
    };
    tokio::spawn(async move {
        let addr = format!("127.0.0.1:{WEB_PORT}");
        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                warn!("web UI could not bind {addr}: {e}");
                return;
            }
        };
        let index = dir.join("index.html");
        let service = ServeDir::new(&dir).not_found_service(ServeFile::new(index));
        let app = Router::new().fallback_service(service);
        tracing::info!("web UI at http://127.0.0.1:{WEB_PORT}/");
        if axum::serve(listener, app).await.is_err() {
            warn!("web UI server exited");
        }
    });
}

fn resolve_web_dir() -> Option<PathBuf> {
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../app/web");
    let installed = homedir().map(|h| h.join(".synapse/web"));
    let candidates = [
        std::env::var_os("SYNAPSE_WEB_DIR").map(PathBuf::from),
        pick_newer_web_dir(&dev, installed.as_deref()),
        Some(dev),
        installed,
    ];
    for dir in candidates.into_iter().flatten() {
        if dir.join("index.html").is_file() {
            return Some(dir);
        }
    }
    None
}

/// Prefer the repo dev bundle when it is newer than the installed copy (local dev).
fn pick_newer_web_dir(dev: &Path, installed: Option<&Path>) -> Option<PathBuf> {
    let dev_index = dev.join("index.html");
    if !dev_index.is_file() {
        return None;
    }
    let Some(inst) = installed else {
        return None;
    };
    let inst_index = inst.join("index.html");
    if !inst_index.is_file() {
        return Some(dev.to_path_buf());
    }
    let dev_m = std::fs::metadata(&dev_index).ok()?.modified().ok()?;
    let inst_m = std::fs::metadata(&inst_index).ok()?.modified().ok()?;
    if dev_m > inst_m {
        Some(dev.to_path_buf())
    } else {
        None
    }
}

fn homedir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_web_bundle_exists() {
        let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/web/index.html");
        assert!(dev.is_file(), "missing dev web bundle at {}", dev.display());
    }
}
