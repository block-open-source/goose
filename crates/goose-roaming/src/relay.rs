//! Relay configuration for the roaming endpoint.
//!
//! Relays give NAT traversal and a fallback path when a direct QUIC route
//! cannot be established. We use a *custom* relay map rather than iroh's default
//! n0 relays so a deployment can point at its own zero-trust relays.
//!
//! Per security review: relay URLs travel in connection cards, but relay
//! *auth credentials* never do — each endpoint must obtain its own.

use iroh::{RelayConfig, RelayMap, RelayMode, RelayUrl};

use crate::error::RoamingError;

/// How the roaming endpoint reaches relays.
#[derive(Debug, Clone, Default)]
pub enum RelaySettings {
    /// Use iroh's built-in n0 production relays. Convenient for getting
    /// started; n0 documents these as rate-limited and not for production.
    #[default]
    N0Default,
    /// Use a custom set of relay URLs, optionally with a bearer auth token
    /// per URL (for gated / zero-trust relays).
    Custom(Vec<RelayEntry>),
    /// No relays: direct connectivity only (LAN / already-reachable hosts).
    Disabled,
}

/// A single relay URL plus optional auth token.
#[derive(Debug, Clone)]
pub struct RelayEntry {
    pub url: String,
    pub auth_token: Option<String>,
    /// Port for the relay's QUIC address discovery (QAD) endpoint. `None`
    /// means iroh's default QAD port — the same assumption iroh makes for its
    /// own default relays. Address discovery is how a NAT'd peer learns its
    /// reflexive address; without it, connections through a custom relay can
    /// never upgrade to a direct path (found by @pstayets' real-NAT harness,
    /// issue #10906).
    pub qad_port: Option<u16>,
}

impl RelayEntry {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            auth_token: None,
            qad_port: None,
        }
    }

    pub fn with_auth(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            auth_token: Some(token.into()),
            qad_port: None,
        }
    }
}

impl RelaySettings {
    /// The relay URLs advertised to peers in connection cards (auth tokens stripped).
    pub fn advertised_urls(&self) -> Vec<String> {
        match self {
            RelaySettings::Custom(entries) => entries.iter().map(|e| e.url.clone()).collect(),
            RelaySettings::N0Default | RelaySettings::Disabled => Vec::new(),
        }
    }

    /// Convert to an iroh [`RelayMode`].
    pub fn to_relay_mode(&self) -> Result<RelayMode, RoamingError> {
        match self {
            RelaySettings::N0Default => Ok(RelayMode::Default),
            RelaySettings::Disabled => Ok(RelayMode::Disabled),
            RelaySettings::Custom(entries) => {
                let mut configs = Vec::with_capacity(entries.len());
                for entry in entries {
                    let url: RelayUrl = entry.url.parse().map_err(|_| {
                        RoamingError::Transport(format!("bad relay url {}", entry.url))
                    })?;
                    // Enable QUIC address discovery: without it a NAT'd peer
                    // never learns its reflexive address and the connection
                    // stays relay-only forever (no direct upgrade). Default
                    // port matches iroh's own relay defaults.
                    let quic = Some(match entry.qad_port {
                        Some(port) => iroh_relay::RelayQuicConfig::new(port),
                        None => iroh_relay::RelayQuicConfig::default(),
                    });
                    let cfg = RelayConfig::new(url, quic);
                    let cfg = match &entry.auth_token {
                        Some(token) => {
                            // Never send a bearer token in cleartext: a
                            // token on an http:// relay would be readable by
                            // anyone on the path.
                            if !entry.url.trim_start().starts_with("https://") {
                                return Err(RoamingError::Transport(format!(
                                    "relay {} has an auth token but is not https; \
                                     refusing to send the token in cleartext",
                                    entry.url
                                )));
                            }
                            cfg.with_auth_token(token.clone())
                        }
                        None => cfg,
                    };
                    configs.push(cfg);
                }
                Ok(RelayMode::Custom(RelayMap::from_iter(configs)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_advertises_no_urls() {
        assert!(RelaySettings::default().advertised_urls().is_empty());
    }

    #[test]
    fn custom_strips_auth_from_advertised() {
        let settings = RelaySettings::Custom(vec![RelayEntry::with_auth(
            "https://relay.example./",
            "secret",
        )]);
        assert_eq!(settings.advertised_urls(), vec!["https://relay.example./"]);
    }

    #[test]
    fn custom_builds_relay_mode() {
        let settings = RelaySettings::Custom(vec![RelayEntry::new("https://relay.example./")]);
        assert!(matches!(
            settings.to_relay_mode().unwrap(),
            RelayMode::Custom(_)
        ));
    }
}
