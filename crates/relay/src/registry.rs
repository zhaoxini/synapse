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
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

pub type FrameTx = mpsc::Sender<Message>;

struct DeviceEntry {
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
        server_tx: FrameTx,
    ) -> Arc<Mutex<Option<FrameTx>>> {
        let slot = Arc::new(Mutex::new(None));
        self.devices.insert(
            device_id.to_string(),
            DeviceEntry {
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
    pub async fn acquire(&self, device_id: &str) -> Option<(FrameTx, Arc<Mutex<Option<FrameTx>>>)> {
        let entry = self.devices.get(device_id)?;
        Some((entry.server_tx.clone(), entry.app_slot.clone()))
    }

    pub async fn device_count(&self) -> usize {
        self.devices.len()
    }

    pub async fn online_ids(&self) -> HashSet<String> {
        self.devices.iter().map(|e| e.key().clone()).collect()
    }

    pub async fn is_online(&self, device_id: &str) -> bool {
        self.devices.contains_key(device_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_and_acquire() {
        let reg = Registry::new();
        let (tx, _rx) = mpsc::channel(4);
        let slot = reg.register("dev1", tx.clone()).await;
        assert!(reg.is_online("dev1").await);
        let (server_tx, got_slot) = reg.acquire("dev1").await.unwrap();
        assert!(Arc::ptr_eq(&slot, &got_slot));
        drop(server_tx);
        reg.unregister("dev1").await;
        assert!(!reg.is_online("dev1").await);
    }
}
