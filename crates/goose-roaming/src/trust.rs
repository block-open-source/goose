//! Local access-control state: which peer keys this node accepts inbound
//! connections from, and which are revoked.
//!
//! Trust is a **mutual, public-key allowlist**. A peer is identified by the key
//! iroh's QUIC-TLS handshake authenticated, and is admitted only if that key is
//! on this node's allowlist. There is no bearer/token mode: sharing a
//! [`crate::ConnectionCard`] grants nothing until the recipient explicitly
//! accepts the sender's key. An accepted peer gets goose's full ACP surface.
//!
//! This is deliberately local, unsigned admin state: it lives on the host under
//! the user's control. Authentication of *who* a peer is comes from the
//! transport; this layer decides *whether* they are authorized.

use std::collections::BTreeSet;

use iroh::EndpointId;
use serde::{Deserialize, Serialize};

/// Persisted trust state: the inbound allowlist plus revocations.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrustBook {
    /// Peer keys allowed to connect.
    allowed: BTreeSet<String>,
    /// Peer keys that are refused regardless of anything else.
    revoked_keys: BTreeSet<String>,
}

impl TrustBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accept inbound connections from `key`. Clears any prior revocation.
    pub fn accept(&mut self, key: &EndpointId) {
        let s = key_str(key);
        self.revoked_keys.remove(&s);
        self.allowed.insert(s);
    }

    /// Stop accepting `key` and record it as revoked so a stale card can't
    /// silently re-add it.
    pub fn revoke_key(&mut self, key: &EndpointId) {
        let s = key_str(key);
        self.allowed.remove(&s);
        self.revoked_keys.insert(s);
    }

    /// Whether `key` is allowed to connect (on the allowlist and not revoked).
    pub fn is_allowed(&self, key: &EndpointId) -> bool {
        let s = key_str(key);
        !self.revoked_keys.contains(&s) && self.allowed.contains(&s)
    }

    pub(crate) fn is_key_revoked(&self, key: &EndpointId) -> bool {
        self.revoked_keys.contains(&key_str(key))
    }

    /// Allowed peer keys, sorted.
    pub fn allowed_keys(&self) -> Vec<String> {
        self.allowed.iter().cloned().collect()
    }

    /// Load the trust book. A missing file is an empty book; a *malformed*
    /// file is a hard error — silently treating corruption as "no one is
    /// allowed" would strand peers, and treating it as "keep going" would be
    /// worse. Callers on the authorization path fail closed on this error.
    pub fn load(path: &std::path::Path) -> Result<Self, std::io::Error> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Persist atomically (unique temp file + rename) so a concurrent reader
    /// on the authorization path never observes a half-written file, and
    /// concurrent writers never truncate each other's in-flight temp file.
    pub fn save(&self, path: &std::path::Path) -> Result<(), std::io::Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }

    /// Read-modify-write the trust book under a cross-process advisory lock.
    ///
    /// Atomic replacement in [`save`] protects readers from partial JSON but
    /// not writers from lost updates: two `goose roam peers` commands (or any
    /// other embedder) each load the whole book, mutate, and save, so the last
    /// writer clobbers the other's change with a stale snapshot —
    /// e.g. a concurrent accept resurrects a peer that was just revoked. This
    /// serializes the whole load+mutate+save so those edits can't race. The
    /// lock is held on a sidecar `.lock` file (never the book itself) and
    /// auto-releases if the holder dies.
    pub fn update(
        path: &std::path::Path,
        mutate: impl FnOnce(&mut Self),
    ) -> Result<Self, std::io::Error> {
        use fs2::FileExt as _;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let lock_path = path.with_extension("json.lock");
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        lock.lock_exclusive()?;

        let result = (|| {
            let mut book = Self::load(path)?;
            mutate(&mut book);
            book.save(path)?;
            Ok(book)
        })();

        let _ = fs2::FileExt::unlock(&lock);
        result
    }
}

fn key_str(key: &EndpointId) -> String {
    key.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    #[test]
    fn allowlist_gates() {
        let mut book = TrustBook::new();
        let key = SecretKey::generate().public();
        assert!(!book.is_allowed(&key));

        book.accept(&key);
        assert!(book.is_allowed(&key));
    }

    #[test]
    fn revocation_removes_and_blocks() {
        let mut book = TrustBook::new();
        let key = SecretKey::generate().public();
        book.accept(&key);
        book.revoke_key(&key);
        assert!(!book.is_allowed(&key));
        assert!(book.is_key_revoked(&key));
    }

    #[test]
    fn accept_clears_prior_revocation() {
        let mut book = TrustBook::new();
        let key = SecretKey::generate().public();
        book.revoke_key(&key);
        book.accept(&key);
        assert!(book.is_allowed(&key));
        assert!(!book.is_key_revoked(&key));
    }

    #[test]
    fn persists_across_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.json");
        let key = SecretKey::generate().public();
        {
            let mut book = TrustBook::new();
            book.accept(&key);
            book.save(&path).unwrap();
        }
        let reloaded = TrustBook::load(&path).unwrap();
        assert!(reloaded.is_allowed(&key));
    }

    #[test]
    fn corrupt_file_is_a_hard_error_not_an_empty_book() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.json");
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(TrustBook::load(&path).is_err());
    }

    #[test]
    fn missing_file_is_an_empty_book() {
        let dir = tempfile::tempdir().unwrap();
        let book = TrustBook::load(&dir.path().join("nope.json")).unwrap();
        assert!(book.allowed_keys().is_empty());
    }

    #[test]
    fn update_persists_the_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.json");
        let key = SecretKey::generate().public();
        TrustBook::update(&path, |book| book.accept(&key)).unwrap();
        assert!(TrustBook::load(&path).unwrap().is_allowed(&key));
    }

    #[test]
    fn concurrent_updates_do_not_lose_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::sync::Arc::new(dir.path().join("trust.json"));
        let keys: Vec<_> = (0..8).map(|_| SecretKey::generate().public()).collect();

        // Each thread does its own lock-guarded load+mutate+save. Without the
        // cross-process lock these interleave and clobber each other; with it,
        // every accepted key must survive.
        let handles: Vec<_> = keys
            .iter()
            .map(|key| {
                let path = std::sync::Arc::clone(&path);
                let key = *key;
                std::thread::spawn(move || {
                    TrustBook::update(&path, |book| book.accept(&key)).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let book = TrustBook::load(&path).unwrap();
        for key in &keys {
            assert!(book.is_allowed(key), "lost an accepted key");
        }
    }
}
