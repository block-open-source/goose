//! A user-managed address book of remote nodes you can connect to.
//!
//! Unlike [`crate::Directory`] (which records connections that actually
//! happened), the [`PeerBook`] holds saved remotes you *may* connect to,
//! addressed by a friendly nickname. Each entry stores the remote's
//! [`ConnectionCard`] — its public identity and how to reach it. A card is
//! **not** a secret and confers no access on its own; a connection only
//! succeeds if the remote has also chosen to accept this node's key.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::card::ConnectionCard;
use crate::error::RoamingError;

/// A single saved remote node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerRecord {
    /// Friendly nickname used to `connect <name>`.
    pub name: String,
    /// The remote's connection card (public identity + reachability).
    pub card: ConnectionCard,
    /// Cached for display without re-decoding.
    pub endpoint_id: String,
    /// Short fingerprint for out-of-band verification.
    pub fingerprint: String,
    pub added_ms: u64,
}

/// A persisted map of nickname -> saved remote.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PeerBook {
    peers: BTreeMap<String, PeerRecord>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl PeerBook {
    /// Load the peer book from `path`, or start empty if it does not exist.
    /// Mutations are flushed back to `path`.
    pub fn load(path: PathBuf) -> Result<Self, RoamingError> {
        let mut book = match std::fs::read(&path) {
            // Surface corrupt JSON instead of starting empty: the next mutating
            // command would flush that empty book back and permanently lose
            // every saved peer card and nickname.
            Ok(bytes) => serde_json::from_slice::<PeerBook>(&bytes)
                .map_err(|e| RoamingError::Io(std::io::Error::other(e)))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => PeerBook::default(),
            Err(e) => return Err(RoamingError::Io(e)),
        };
        book.path = Some(path);
        Ok(book)
    }

    /// Save a remote under `name` from its shared card string. Returns an error
    /// if the card is malformed. Overwrites an existing entry with the same
    /// name (used to refresh a card whose relays changed).
    pub fn save(&mut self, name: &str, card_str: &str, now_ms: u64) -> Result<(), RoamingError> {
        let card = ConnectionCard::decode(card_str)?;
        let record = PeerRecord {
            name: name.to_string(),
            endpoint_id: card.endpoint_id.to_string(),
            fingerprint: card.fingerprint(),
            card,
            added_ms: now_ms,
        };
        self.peers.insert(name.to_string(), record);
        self.flush()
    }

    /// Remove a saved remote. Returns whether it existed.
    pub fn remove(&mut self, name: &str) -> Result<bool, RoamingError> {
        let existed = self.peers.remove(name).is_some();
        if existed {
            self.flush()?;
        }
        Ok(existed)
    }

    /// Rename a saved remote. Returns an error if `from` is missing or `to`
    /// already exists.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), RoamingError> {
        if self.peers.contains_key(to) {
            return Err(RoamingError::Card(format!("peer `{to}` already exists")));
        }
        let mut record = self
            .peers
            .remove(from)
            .ok_or_else(|| RoamingError::Card(format!("no peer named `{from}`")))?;
        record.name = to.to_string();
        self.peers.insert(to.to_string(), record);
        self.flush()
    }

    /// Look up a saved remote by nickname.
    pub fn get(&self, name: &str) -> Option<&PeerRecord> {
        self.peers.get(name)
    }

    /// All saved remotes, sorted by nickname.
    pub fn list(&self) -> Vec<&PeerRecord> {
        self.peers.values().collect()
    }

    /// Read-modify-write the peer book under a cross-process advisory lock.
    ///
    /// Mirrors [`crate::TrustBook::update`]: atomic replacement protects
    /// readers from partial JSON but not writers from lost updates — two
    /// concurrent `goose roam peers`/pairing commands each load the whole
    /// book, mutate, and save, so the last writer clobbers the other's add,
    /// remove, or rename. The lock is held on a sidecar `.lock` file and
    /// auto-releases if the holder dies.
    pub fn update<T>(
        path: PathBuf,
        mutate: impl FnOnce(&mut Self) -> Result<T, RoamingError>,
    ) -> Result<T, RoamingError> {
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
            mutate(&mut book)
        })();

        let _ = fs2::FileExt::unlock(&lock);
        result
    }

    fn flush(&self) -> Result<(), RoamingError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| RoamingError::Card(format!("serialize peer book: {e}")))?;
        write_file(path, &json)
    }
}

fn write_file(path: &Path, contents: &[u8]) -> Result<(), RoamingError> {
    // Unique per writer so concurrent flushes cannot rename each other's
    // half-written bytes into place or fail on a stolen temporary file.
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::RoamingIdentity;

    fn make_card() -> String {
        let host = RoamingIdentity::generate();
        ConnectionCard::new(host.public_key(), vec!["https://relay.example./".into()])
            .encode()
            .unwrap()
    }

    #[test]
    fn corrupt_book_is_an_error_not_an_empty_book() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.json");

        let mut book = PeerBook::load(path.clone()).unwrap();
        book.save("work", &make_card(), 1_000).unwrap();

        std::fs::write(&path, b"{ truncated").unwrap();

        assert!(
            PeerBook::load(path).is_err(),
            "a corrupt peer book must fail to load; starting empty would let the \
             next mutating command flush the empty book back and lose every peer"
        );
    }

    #[test]
    fn save_get_remove() {
        let dir = tempfile::tempdir().unwrap();
        let mut book = PeerBook::load(dir.path().join("peers.json")).unwrap();
        let card = make_card();

        book.save("work", &card, 1_000).unwrap();
        let rec = book.get("work").unwrap();
        assert_eq!(rec.name, "work");
        assert!(!rec.fingerprint.is_empty());

        assert!(book.remove("work").unwrap());
        assert!(book.get("work").is_none());
        assert!(!book.remove("work").unwrap());
    }

    #[test]
    fn rename_rules() {
        let dir = tempfile::tempdir().unwrap();
        let mut book = PeerBook::load(dir.path().join("peers.json")).unwrap();
        book.save("a", &make_card(), 1).unwrap();
        book.save("b", &make_card(), 1).unwrap();

        assert!(book.rename("a", "b").is_err()); // target exists
        assert!(book.rename("missing", "c").is_err()); // source missing
        book.rename("a", "c").unwrap();
        assert!(book.get("a").is_none());
        assert_eq!(book.get("c").unwrap().name, "c");
    }

    #[test]
    fn persists_across_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.json");
        {
            let mut book = PeerBook::load(path.clone()).unwrap();
            book.save("work", &make_card(), 1).unwrap();
        }
        let book = PeerBook::load(path).unwrap();
        assert_eq!(book.list().len(), 1);
        assert_eq!(book.get("work").unwrap().name, "work");
    }

    #[test]
    fn rejects_malformed_card() {
        let dir = tempfile::tempdir().unwrap();
        let mut book = PeerBook::load(dir.path().join("peers.json")).unwrap();
        assert!(book.save("bad", "not-a-card", 1).is_err());
    }
}
