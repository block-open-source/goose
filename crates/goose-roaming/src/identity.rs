//! Persisted roaming node identity.
//!
//! A roaming node is identified by an ed25519 keypair. The public key doubles
//! as the iroh [`EndpointId`], so the identity is self-certifying: iroh proves
//! at the QUIC-TLS handshake that a peer holds the secret for the id it claims.
//!
//! The secret key is persisted as hex in a `0600` file inside goose's config
//! directory. This mirrors the storage approach used by a sibling production
//! iroh project.

use std::path::{Path, PathBuf};

use iroh::{PublicKey, SecretKey};

use crate::error::RoamingError;

const KEY_FILE_NAME: &str = "roaming_node_key";

/// A roaming node's long-lived identity.
#[derive(Clone)]
pub struct RoamingIdentity {
    secret: SecretKey,
}

impl RoamingIdentity {
    /// Wrap an existing secret key.
    pub fn from_secret(secret: SecretKey) -> Self {
        Self { secret }
    }

    /// Generate a fresh, ephemeral identity (not persisted).
    pub fn generate() -> Self {
        Self {
            secret: SecretKey::generate(),
        }
    }

    /// Load the node identity from `path`, creating and persisting a new one if
    /// the file does not exist.
    ///
    /// First creation is serialized on a sidecar advisory lock and lands via a
    /// fully written temporary file renamed into place, so racing processes
    /// agree on one key and a crash mid-write can never leave a truncated key
    /// at the final path (which would permanently disable roaming until the
    /// user deletes it — changing the endpoint ID peers trusted).
    pub fn load_or_create(path: &Path) -> Result<Self, RoamingError> {
        use fs2::FileExt as _;

        if path.exists() {
            return Self::load(path);
        }

        let parent = path.parent().ok_or_else(|| {
            RoamingError::Identity(format!("key path {} has no parent", path.display()))
        })?;
        ensure_private_dir(parent)?;

        let lock_path = path.with_extension("lock");
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        lock.lock_exclusive()?;

        let result = (|| {
            // A racer may have created the key while we waited for the lock.
            if path.exists() {
                return Self::load(path);
            }
            let identity = Self::generate();
            identity.save(path)?;
            Ok(identity)
        })();

        let _ = fs2::FileExt::unlock(&lock);
        result
    }

    /// Load the node identity from a hex-encoded key file.
    pub fn load(path: &Path) -> Result<Self, RoamingError> {
        ensure_private_file(path)?;
        let hex = std::fs::read_to_string(path)?;
        let bytes = decode_hex_key(hex.trim())?;
        Ok(Self {
            secret: SecretKey::from_bytes(&bytes),
        })
    }

    /// Persist the identity to `path` with `0600` permissions.
    pub fn save(&self, path: &Path) -> Result<(), RoamingError> {
        let parent = path.parent().ok_or_else(|| {
            RoamingError::Identity(format!("key path {} has no parent", path.display()))
        })?;
        ensure_private_dir(parent)?;
        let encoded = encode_hex_key(&self.secret.to_bytes());
        write_atomically(path, encoded.as_bytes())?;
        ensure_private_file(path)?;
        Ok(())
    }

    /// The iroh secret key.
    pub(crate) fn secret_key(&self) -> &SecretKey {
        &self.secret
    }

    /// The node's public key / [`iroh::EndpointId`].
    pub fn public_key(&self) -> PublicKey {
        self.secret.public()
    }
}

/// The default node key path inside goose's config directory.
pub fn default_key_path(config_dir: &Path) -> PathBuf {
    config_dir.join(KEY_FILE_NAME)
}

fn encode_hex_key(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn decode_hex_key(hex: &str) -> Result<[u8; 32], RoamingError> {
    if hex.len() != 64 {
        return Err(RoamingError::Identity(format!(
            "node key must be 64 hex chars, got {}",
            hex.len()
        )));
    }
    let mut bytes = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk)
            .map_err(|_| RoamingError::Identity("node key is not valid utf-8".into()))?;
        bytes[i] = u8::from_str_radix(s, 16)
            .map_err(|_| RoamingError::Identity("node key has invalid hex".into()))?;
    }
    Ok(bytes)
}

fn write_atomically(path: &Path, contents: &[u8]) -> Result<(), RoamingError> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(unix)]
fn ensure_private_dir(dir: &Path) -> Result<(), RoamingError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    let mut perms = std::fs::metadata(dir)?.permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(dir, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_dir(dir: &Path) -> Result<(), RoamingError> {
    std::fs::create_dir_all(dir)?;
    Ok(())
}

#[cfg(unix)]
fn ensure_private_file(path: &Path) -> Result<(), RoamingError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_file(_path: &Path) -> Result<(), RoamingError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_hex_key() {
        let secret = SecretKey::generate();
        let bytes = secret.to_bytes();
        let hex = encode_hex_key(&bytes);
        assert_eq!(decode_hex_key(&hex).unwrap(), bytes);
    }

    #[test]
    fn load_or_create_persists_stable_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = default_key_path(dir.path());

        let first = RoamingIdentity::load_or_create(&path).unwrap();
        let second = RoamingIdentity::load_or_create(&path).unwrap();

        assert_eq!(first.public_key(), second.public_key());
    }

    #[test]
    fn rejects_malformed_key() {
        assert!(decode_hex_key("nothex").is_err());
        assert!(decode_hex_key("ab").is_err());
    }
}
