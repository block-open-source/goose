//! An out-of-band directory of roaming peers.
//!
//! There is deliberately **no gossip**: the directory is built purely from
//! connections this node observes. Inbound connections are recorded when a peer
//! is authorized; outbound connections are recorded when this node dials a
//! remote agent. This gives `goose roam list`-style visibility without any
//! ambient network discovery.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use iroh::EndpointId;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

/// Which way a connection was established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// A remote peer connected to us.
    Inbound,
    /// We connected to a remote peer.
    Outbound,
}

/// A single directory entry describing a peer we have interacted with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerEntry {
    pub endpoint_id: String,
    pub label: Option<String>,
    pub direction: Direction,
    /// Best-effort agent id reported during the handshake.
    pub agent_id: Option<String>,
    pub first_seen_ms: u64,
    pub last_seen_ms: u64,
    /// Whether a session is currently active with this peer.
    pub connected: bool,
    /// Live connections with this peer in the current process. Not persisted:
    /// only the process owning the endpoint knows what is live, so a restart
    /// starts from zero rather than trusting a stale flag from disk.
    #[serde(skip)]
    live_connections: u32,
}

/// A shared directory of peers, optionally persisted to disk so that a separate
/// process (e.g. `goose roam list`) can read what a running `share` has seen.
#[derive(Clone, Default)]
pub struct Directory {
    inner: Arc<Mutex<HashMap<String, PeerEntry>>>,
    path: Option<PathBuf>,
}

impl Directory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a directory backed by a JSON file at `path`, loading any existing
    /// entries. All mutations are flushed back to the file (best effort).
    pub fn persistent(path: PathBuf) -> Self {
        let entries = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<PeerEntry>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|mut e| {
                // A persisted `connected` flag outlives the process that owned
                // the connection (crash, SIGKILL, reboot). Only this process's
                // own observations may claim a peer is live.
                e.connected = false;
                (e.endpoint_id.clone(), e)
            })
            .collect::<HashMap<_, _>>();
        Self {
            inner: Arc::new(Mutex::new(entries)),
            path: Some(path),
        }
    }

    /// Like [`persistent`](Self::persistent), for the process that *owns* the
    /// roaming endpoint (holds the exclusive endpoint lock). Because no other
    /// owner can be alive, any persisted `connected` flags are stale by
    /// definition — from a crash, SIGKILL, or reboot — so the cleared state is
    /// flushed straight back to disk, making `goose roam connections` stop
    /// reporting phantom live peers immediately after a restart.
    pub fn persistent_owned(path: PathBuf) -> Self {
        let dir = Self::persistent(path.clone());
        // The directory was just constructed and is not yet shared, so the
        // lock is always free (`try_lock` cannot fail); this also stays safe
        // inside an async runtime where a blocking lock would panic.
        if let Ok(map) = dir.inner.try_lock() {
            let mut entries: Vec<&PeerEntry> = map.values().collect();
            entries.sort_by_key(|e| std::cmp::Reverse(e.last_seen_ms));
            if let Ok(json) = serde_json::to_vec_pretty(&entries) {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
                if std::fs::write(&tmp, json).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            }
        }
        dir
    }

    /// Read the persisted directory at `path` without holding the endpoint.
    pub fn read_persisted(path: &std::path::Path) -> Vec<PeerEntry> {
        let mut entries = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<PeerEntry>>(&bytes).ok())
            .unwrap_or_default();
        entries.sort_by_key(|e| std::cmp::Reverse(e.last_seen_ms));
        entries
    }

    /// Flush this node's view to disk, merged with whatever other processes
    /// have written (a share and an outbound `connect`/`delegate` may run
    /// concurrently). The whole reload-merge-write runs under a cross-process
    /// advisory lock and lands via a unique temp file renamed into place, so
    /// concurrent flushes can neither interleave bytes nor drop each other's
    /// observations.
    async fn flush(&self, map: &HashMap<String, PeerEntry>) {
        use fs2::FileExt as _;

        let Some(path) = &self.path else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let lock_path = path.with_extension("json.lock");
        let Ok(lock) = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
        else {
            return;
        };
        if lock.lock_exclusive().is_err() {
            return;
        }

        let mut merged: HashMap<String, PeerEntry> = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<PeerEntry>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.endpoint_id.clone(), e))
            .collect();
        for entry in map.values() {
            match merged.get(&entry.endpoint_id) {
                // Keep the on-disk entry unless this process owns live
                // connections with the peer or has a strictly fresher
                // observation. Equal timestamps mean this process merely
                // loaded the entry and has nothing new to say — overwriting
                // would clobber another process's live state with our stale
                // snapshot.
                Some(existing)
                    if entry.live_connections == 0
                        && existing.last_seen_ms >= entry.last_seen_ms => {}
                _ => {
                    merged.insert(entry.endpoint_id.clone(), entry.clone());
                }
            }
        }

        let mut entries: Vec<&PeerEntry> = merged.values().collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.last_seen_ms));
        if let Ok(json) = serde_json::to_vec_pretty(&entries) {
            let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }

        let _ = fs2::FileExt::unlock(&lock);
    }

    /// Record the start of a connection, creating or updating the entry.
    pub(crate) async fn record_connect(
        &self,
        endpoint_id: EndpointId,
        label: Option<String>,
        direction: Direction,
        agent_id: Option<String>,
        now_ms: u64,
    ) {
        let key = endpoint_id.to_string();
        let mut map = self.inner.lock().await;
        map.entry(key.clone())
            .and_modify(|e| {
                e.last_seen_ms = now_ms;
                e.connected = true;
                e.live_connections = e.live_connections.saturating_add(1);
                if label.is_some() {
                    e.label = label.clone();
                }
                if agent_id.is_some() {
                    e.agent_id = agent_id.clone();
                }
            })
            .or_insert(PeerEntry {
                endpoint_id: key,
                label,
                direction,
                agent_id,
                first_seen_ms: now_ms,
                last_seen_ms: now_ms,
                connected: true,
                live_connections: 1,
            });
        self.flush(&map).await;
    }

    /// Record that a connection with a peer has ended. The peer is only marked
    /// disconnected when its *last* live connection ends; a peer with several
    /// simultaneous sessions stays `connected` until all of them close.
    pub(crate) async fn record_disconnect(&self, endpoint_id: EndpointId, now_ms: u64) {
        let key = endpoint_id.to_string();
        let mut map = self.inner.lock().await;
        if let Some(entry) = map.get_mut(&key) {
            entry.live_connections = entry.live_connections.saturating_sub(1);
            entry.connected = entry.live_connections > 0;
            entry.last_seen_ms = now_ms;
        }
        self.flush(&map).await;
    }

    /// Snapshot the directory, most-recently-seen first.
    pub async fn list(&self) -> Vec<PeerEntry> {
        let map = self.inner.lock().await;
        let mut entries: Vec<PeerEntry> = map.values().cloned().collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.last_seen_ms));
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    #[tokio::test]
    async fn records_and_lists() {
        let dir = Directory::new();
        let peer = SecretKey::generate().public();
        dir.record_connect(
            peer,
            Some("laptop".into()),
            Direction::Inbound,
            Some("agent-1".into()),
            1_000,
        )
        .await;

        let list = dir.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].label.as_deref(), Some("laptop"));
        assert!(list[0].connected);

        dir.record_disconnect(peer, 2_000).await;
        let after = dir.list().await;
        assert!(!after[0].connected);
        assert_eq!(after[0].last_seen_ms, 2_000);
    }
}
