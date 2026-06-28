// Device registry for the relay.
//
// A device = a local synapse-server that maintains an outbound uplink WS to the
// relay. While online it owns:
//   * `server_tx` — relay pushes app-originated frames here, uplink forwards
//     them to the server socket.
//   * `app_slot`  — when an app is linked, its `to_app` sender lives here so
//     the uplink can deliver server-originated frames back to the app.
//
// At most one app is linked to a device at a time (single-app product model).

use axum::extract::ws::Message;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

pub type FrameTx = mpsc::Sender<Message>;

struct DeviceEntry {
    token: Option<String>,
    server_tx: FrameTx,
    /// Sender to the currently linked app's socket (if any).
    app_slot: Arc<Mutex<Option<FrameTx>>>,
}

#[derive(Clone, Default)]
pub struct Registry {
    devices: Arc<DashMap<String, DeviceEntry>>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            devices: Arc::new(DashMap::new()),
        }
    }

    /// Register an online device and return its app-slot handle.
    pub async fn register(
        &self,
        device_id: &str,
        token: Option<String>,
        server_tx: FrameTx,
    ) -> Arc<Mutex<Option<FrameTx>>> {
        let slot = Arc::new(Mutex::new(None));
        self.devices.insert(
            device_id.to_string(),
            DeviceEntry {
                token,
                server_tx,
                app_slot: slot.clone(),
            },
        );
        slot
    }

    pub async fn unregister(&self, device_id: &str) {
        self.devices.remove(device_id);
    }

    /// Authorize a downlink and return (server sender, app slot). The downlink
    /// links itself into the slot so the uplink can route frames to it.
    pub async fn acquire(
        &self,
        device_id: &str,
        token: Option<&str>,
    ) -> Option<(FrameTx, Arc<Mutex<Option<FrameTx>>>)> {
        let entry = self.devices.get(device_id)?;
        if !token_match(entry.token.as_deref(), token) {
            return None;
        }
        Some((entry.server_tx.clone(), entry.app_slot.clone()))
    }

    pub async fn device_count(&self) -> usize {
        self.devices.len()
    }
}

fn token_match(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y,
        (None, _) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx() -> FrameTx {
        tokio::sync::mpsc::channel::<Message>(1).0
    }

    #[tokio::test]
    async fn register_acquire_match() {
        let r = Registry::new();
        r.register("d1", Some("secret".into()), tx()).await;
        assert_eq!(r.device_count().await, 1);
        // matching token -> Some
        assert!(r.acquire("d1", Some("secret")).await.is_some());
        // wrong token -> None
        assert!(r.acquire("d1", Some("nope")).await.is_none());
    }

    #[tokio::test]
    async fn acquire_offline_is_none() {
        let r = Registry::new();
        assert!(r.acquire("ghost", Some("x")).await.is_none());
    }

    #[tokio::test]
    async fn unregister_drops_device() {
        let r = Registry::new();
        r.register("d2", Some("t".into()), tx()).await;
        assert_eq!(r.device_count().await, 1);
        r.unregister("d2").await;
        assert_eq!(r.device_count().await, 0);
        assert!(r.acquire("d2", Some("t")).await.is_none());
    }

    #[tokio::test]
    async fn no_device_token_allows_any() {
        let r = Registry::new();
        r.register("d3", None, tx()).await;
        // When the server sets no per-device token, any app token is accepted
        // (relay-wide API token gates separately).
        assert!(r.acquire("d3", Some("anything")).await.is_some());
        assert!(r.acquire("d3", None).await.is_some());
    }

    #[test]
    fn token_match_logic() {
        assert!(token_match(Some("a"), Some("a")));
        assert!(!token_match(Some("a"), Some("b")));
        assert!(token_match(None, Some("x")));
        assert!(!token_match(Some("a"), None));
    }
}
