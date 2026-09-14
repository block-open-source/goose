//! The roaming handshake exchanged on a freshly-accepted bi-stream, before the
//! stream is handed to ACP.
//!
//! There is **no capability token**. A connecting node is identified by the
//! public key that iroh's QUIC-TLS handshake already authenticated
//! (`connection.remote_id()`), and the host authorizes purely by whether that
//! key is on its allowlist. Trust is mutual and key-based: you exchange
//! [`crate::ConnectionCard`]s out of band and each side chooses to accept the
//! other.
//!
//! Flow:
//! 1. Client opens a bi-stream and sends [`ClientHello`] (just a display label;
//!    its identity is already proven by the transport).
//! 2. Host checks the authenticated remote key against its allowlist/revocations
//!    and replies with [`HostAck`].
//! 3. On accept, both sides treat the remainder of the stream as an ACP byte
//!    stream.

use serde::{Deserialize, Serialize};

/// First message a connecting client sends. Carries no credential — the
/// client's identity is the key the transport authenticated.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClientHello {
    /// Human-readable client label for the host's directory (best-effort, not
    /// trusted for authorization).
    pub label: Option<String>,
}

/// Host's response to a [`ClientHello`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostAck {
    /// Connection accepted; the client gets goose's full ACP surface.
    Accepted { agent_id: String },
    /// Connection refused with a coarse reason code.
    Rejected { code: String },
}

impl ClientHello {
    pub fn new(label: Option<String>) -> Self {
        Self { label }
    }
}
