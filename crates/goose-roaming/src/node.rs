//! The roaming node: owns the iroh [`Endpoint`] and [`Router`], hosts agents
//! over the `goose-acp/1` ALPN, and dials remote agents as a client.

use std::sync::Arc;

use futures::io::{AsyncRead, AsyncWrite};
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler, Router},
    Endpoint, EndpointId,
};
use tokio::sync::Mutex;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::card::ConnectionCard;
use crate::directory::{Direction, Directory};
use crate::error::RoamingError;
use crate::frame::{read_frame, write_frame};
use crate::handshake::{ClientHello, HostAck};
use crate::identity::RoamingIdentity;
use crate::relay::RelaySettings;
use crate::trust::TrustBook;

/// ALPN identifying the goose ACP-over-iroh protocol.
pub const ROAMING_ACP_ALPN: &[u8] = b"goose-acp/1";

/// Cap on the handshake phase (open bi-stream + read the client hello). A peer
/// that connects and then stalls without completing the handshake is dropped
/// rather than parking the accept task indefinitely (Slowloris guard).
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How often a sharing node re-checks the persisted allowlist to force-close
/// connections for revoked keys. Armed automatically by [`RoamingNode::share`].
const DEFAULT_REVOCATION_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// Serves an accepted, authorized ACP byte stream. Implemented by the
/// integration layer (e.g. `goose-cli`) so this crate does not depend on the
/// concrete agent/session machinery.
pub trait AcpStreamServer: Send + Sync + 'static {
    /// Drive the ACP protocol to completion over the given stream for the
    /// accepted peer identified by `client`.
    fn serve_stream(
        &self,
        client: EndpointId,
        recv: Box<dyn AsyncRead + Send + Unpin>,
        send: Box<dyn AsyncWrite + Send + Unpin>,
    ) -> futures::future::BoxFuture<'static, anyhow::Result<()>>;

    /// A stable, human-facing id for the agent being shared, surfaced to
    /// clients in the handshake ack.
    fn agent_id(&self) -> String;
}

/// Configuration for binding a roaming node.
///
/// For the common case use [`RoamingConfig::new`] and the `with_*` chainers,
/// which default to iroh's public relays, an empty allowlist (accepts no
/// one), and an in-memory directory:
///
/// ```no_run
/// use goose_roaming::{RoamingConfig, RoamingIdentity, RoamingNode};
/// # async fn f() -> anyhow::Result<()> {
/// let node = RoamingNode::bind(RoamingConfig::new(RoamingIdentity::generate())).await?;
/// # Ok(()) }
/// ```
pub struct RoamingConfig {
    pub identity: RoamingIdentity,
    pub relay: RelaySettings,
    pub trust: TrustBook,
    /// Optional path to the persisted trust allowlist. When set, it is
    /// re-read on every inbound connection so `peers accept`/`revoke` from a
    /// separate process take effect against a running `share` without a
    /// restart. When `None` only the in-memory `trust` is consulted.
    pub trust_path: Option<std::path::PathBuf>,
    /// Directory used to track observed connections. Defaults to an in-memory
    /// directory; pass [`Directory::persistent`] to make `roam list` work from
    /// a separate process.
    pub directory: Directory,
    /// Optional explicit socket address to bind the QUIC endpoint to. When set
    /// with relays disabled, the default IP transports are cleared first so a
    /// single-family local path is used — iroh's multipath negotiation
    /// otherwise stalls (`MultipathNotNegotiated`) when both the specified IPv4
    /// and a default `[::]` IPv6 socket are candidates with no relay fallback.
    pub bind_addr: Option<std::net::SocketAddr>,
    /// Override the CA trust used for relay TLS. `None` (the default) uses
    /// the system roots. Tests pass `CaTlsConfig::insecure_skip_verify()`
    /// (available under iroh's `test-utils` feature) to run against a local
    /// self-signed relay.
    pub relay_tls: Option<iroh::tls::CaTlsConfig>,
}

impl RoamingConfig {
    /// A config for `identity` with sensible defaults: iroh's public relays,
    /// an empty trust allowlist (accepts no one until a peer key is accepted),
    /// an in-memory directory, and no explicit bind address.
    pub fn new(identity: RoamingIdentity) -> Self {
        Self {
            identity,
            relay: RelaySettings::N0Default,
            trust: TrustBook::new(),
            trust_path: None,
            directory: Directory::new(),
            bind_addr: None,
            relay_tls: None,
        }
    }

    /// Use a specific relay configuration (default: iroh's public relays).
    pub fn with_relay(mut self, relay: RelaySettings) -> Self {
        self.relay = relay;
        self
    }

    /// Bind the QUIC endpoint to a specific socket address.
    pub fn with_bind_addr(mut self, addr: std::net::SocketAddr) -> Self {
        self.bind_addr = Some(addr);
        self
    }
}

/// A bound roaming node.
pub struct RoamingNode {
    endpoint: Endpoint,
    router: Mutex<Option<Router>>,
    trust: Arc<Mutex<TrustBook>>,
    trust_path: Option<std::path::PathBuf>,
    directory: Directory,
    relay: RelaySettings,
    /// Live authorized inbound connections, by peer key. Lets revocation reach
    /// into the open data plane: a key that leaves the allowlist gets its
    /// connections force-closed, not just refused on the next dial.
    live: Mutex<std::collections::HashMap<EndpointId, Vec<Connection>>>,
    revocation_watcher: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl RoamingNode {
    /// Bind the iroh endpoint. Does not start accepting until [`Self::share`]
    /// (or a manual router) is set up.
    pub async fn bind(config: RoamingConfig) -> Result<Arc<Self>, RoamingError> {
        let relay_mode = config.relay.to_relay_mode()?;
        let relay = config.relay.clone();
        let relays_disabled = matches!(config.relay, RelaySettings::Disabled);
        let mut builder = Endpoint::builder(iroh::endpoint::presets::Minimal)
            .secret_key(config.identity.secret_key().clone())
            .relay_mode(relay_mode);
        if let Some(addr) = config.bind_addr {
            if relays_disabled && addr.is_ipv4() {
                builder = builder.clear_ip_transports();
            }
            builder = builder
                .bind_addr(addr)
                .map_err(|e| RoamingError::Transport(format!("invalid bind address: {e}")))?;
        }
        if let Some(relay_tls) = config.relay_tls {
            builder = builder.ca_tls_config(relay_tls);
        }
        let endpoint = builder
            .bind()
            .await
            .map_err(|e| RoamingError::Transport(format!("failed to bind endpoint: {e}")))?;

        Ok(Arc::new(Self {
            endpoint,
            router: Mutex::new(None),
            trust: Arc::new(Mutex::new(config.trust)),
            trust_path: config.trust_path,
            directory: config.directory,
            relay,
            live: Mutex::new(std::collections::HashMap::new()),
            revocation_watcher: Mutex::new(None),
        }))
    }

    /// The node's public key / endpoint id.
    pub fn endpoint_id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// Access the underlying iroh endpoint (advanced use).
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Shared trust book (for CLI commands to inspect/mutate).
    pub fn trust(&self) -> Arc<Mutex<TrustBook>> {
        self.trust.clone()
    }

    /// Start accepting inbound ACP connections, serving each authorized stream
    /// via `server`. Returns once the router is spawned; it runs in the
    /// background until [`Self::shutdown`].
    ///
    /// If a `trust_path` is configured, live revocation is armed automatically:
    /// a watcher force-closes existing connections when their key leaves the
    /// allowlist. This is a crate invariant, not something callers opt into —
    /// a share that only checked trust at connect time would let a revoked
    /// peer keep an already-open session.
    pub async fn share(
        self: &Arc<Self>,
        server: Arc<dyn AcpStreamServer>,
    ) -> Result<(), RoamingError> {
        let handler = RoamingAcpHandler {
            node: self.clone(),
            server,
        };
        let router = Router::builder(self.endpoint.clone())
            .accept(ROAMING_ACP_ALPN, handler)
            .spawn();
        *self.router.lock().await = Some(router);
        self.watch_revocations(DEFAULT_REVOCATION_POLL).await;
        Ok(())
    }

    /// Wait (up to `timeout`) for the endpoint to contact a relay and be
    /// reachable. Returns `true` if it came online.
    pub async fn wait_online(&self, timeout: std::time::Duration) -> bool {
        tokio::time::timeout(timeout, self.endpoint.online())
            .await
            .is_ok()
    }

    /// The endpoint's currently-known relay URLs, read from its live address.
    /// These are what let a client reach this node when no static relay URLs
    /// are configured (e.g. under [`RelaySettings::N0Default`]).
    pub fn live_relay_urls(&self) -> Vec<String> {
        self.endpoint
            .addr()
            .addrs
            .into_iter()
            .filter_map(|addr| match addr {
                iroh::TransportAddr::Relay(url) => Some(url.to_string()),
                _ => None,
            })
            .collect()
    }

    /// Produce this node's [`ConnectionCard`]: its public identity plus the
    /// relay URLs a peer needs to reach it. The card carries nothing secret and
    /// grants no access — a peer must also be accepted into this node's trust
    /// allowlist before it can connect.
    ///
    /// The relays advertised merge the configured relays with any live relay the
    /// endpoint has since registered with. Call [`Self::wait_online`] first so a
    /// live relay URL is available.
    pub fn card(&self) -> ConnectionCard {
        let mut relay_urls = self.relay.advertised_urls();
        for url in self.live_relay_urls() {
            if !relay_urls.contains(&url) {
                relay_urls.push(url);
            }
        }
        ConnectionCard::new(self.endpoint_id(), relay_urls)
    }

    /// Cleanly shut the router and endpoint down.
    pub async fn shutdown(&self) -> Result<(), RoamingError> {
        if let Some(watcher) = self.revocation_watcher.lock().await.take() {
            watcher.abort();
        }
        if let Some(router) = self.router.lock().await.take() {
            router
                .shutdown()
                .await
                .map_err(|e| RoamingError::Transport(format!("router shutdown: {e}")))?;
        }
        self.endpoint.close().await;
        Ok(())
    }

    async fn register_live(&self, client: EndpointId, connection: Connection) {
        self.live
            .lock()
            .await
            .entry(client)
            .or_default()
            .push(connection);
    }

    async fn unregister_live(&self, client: EndpointId, connection: &Connection) {
        let mut live = self.live.lock().await;
        if let Some(conns) = live.get_mut(&client) {
            conns.retain(|c| c.stable_id() != connection.stable_id());
            if conns.is_empty() {
                live.remove(&client);
            }
        }
    }

    /// Force-close any live connections whose peer key is no longer allowed by
    /// `trust`. Revocation reaches into the open data plane: the allowlist is
    /// not just a gate on new dials, it is authority over connections that
    /// already exist. Returns the number of connections closed.
    pub async fn enforce_trust(&self, trust: &TrustBook) -> usize {
        let to_close: Vec<(EndpointId, Vec<Connection>)> = {
            let mut live = self.live.lock().await;
            let revoked: Vec<EndpointId> = live
                .keys()
                .filter(|key| !trust.is_allowed(key))
                .copied()
                .collect();
            revoked
                .into_iter()
                .filter_map(|key| live.remove(&key).map(|conns| (key, conns)))
                .collect()
        };
        let mut closed = 0;
        for (client, conns) in to_close {
            for conn in conns {
                conn.close(0u32.into(), b"revoked");
                closed += 1;
                // One disconnect per closed connection so the directory's
                // per-peer live count reaches zero.
                self.directory.record_disconnect(client, now_ms()).await;
            }
            tracing::info!(%client, "roaming: force-closed live connection(s) for revoked key");
        }
        closed
    }

    /// Watch the persisted trust allowlist and force-close live connections
    /// for any key that leaves it. Complements the per-connection re-read in
    /// the accept path: that gates *new* dials, this revokes *existing* ones.
    /// No-op unless a `trust_path` is configured.
    pub async fn watch_revocations(self: &Arc<Self>, poll: std::time::Duration) {
        let Some(path) = self.trust_path.clone() else {
            return;
        };
        let node = Arc::downgrade(self);
        let handle = tokio::spawn(async move {
            let mut last_modified: Option<std::time::SystemTime> = None;
            loop {
                tokio::time::sleep(poll).await;
                let Some(node) = node.upgrade() else { break };
                let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                if modified == last_modified {
                    continue;
                }
                match TrustBook::load(&path) {
                    Ok(book) => {
                        // Only advance the watermark on a successful load so a
                        // transient failure is retried on the next poll rather
                        // than leaving revoked connections open.
                        last_modified = modified;
                        node.enforce_trust(&book).await;
                    }
                    Err(e) => {
                        tracing::warn!("roaming: trust reload failed in revocation watcher: {e}");
                    }
                }
            }
        });
        if let Some(old) = self.revocation_watcher.lock().await.replace(handle) {
            old.abort();
        }
    }

    /// Dial a remote node using its [`ConnectionCard`], returning the authorized
    /// bi-stream halves ready to feed to an ACP client transport.
    ///
    /// The dial target is reconstructed from the card (endpoint id + relay
    /// URLs). The connection only succeeds if the remote has accepted *this*
    /// node's key into its allowlist. Use [`Self::connect_with_addr`] when the
    /// caller already has a dialable [`EndpointAddr`] (e.g. a direct LAN address
    /// learned out of band).
    pub async fn connect(
        &self,
        card: &ConnectionCard,
        label: Option<String>,
    ) -> Result<RoamingClientStream, RoamingError> {
        let addr = card.endpoint_addr()?;
        self.connect_with_addr(addr, label).await
    }

    /// Dial a remote node at an explicit [`EndpointAddr`]. Authorization happens
    /// on the remote side purely by this node's authenticated key.
    pub async fn connect_with_addr(
        &self,
        addr: iroh::EndpointAddr,
        label: Option<String>,
    ) -> Result<RoamingClientStream, RoamingError> {
        let conn = self
            .endpoint
            .connect(addr, ROAMING_ACP_ALPN)
            .await
            .map_err(|e| RoamingError::Transport(format!("connect failed: {e}")))?;

        // Bound the client side of the handshake too: a host that accepts the
        // connection but never acks (or stalls mid-frame) must not park the
        // dialer forever. Mirrors the host-side Slowloris guard.
        let handshake = async {
            let (mut send, mut recv) = conn
                .open_bi()
                .await
                .map_err(|e| RoamingError::Transport(format!("open_bi failed: {e}")))?;

            let hello = ClientHello::new(label);
            let hello_bytes = serde_json::to_vec(&hello)
                .map_err(|e| RoamingError::Transport(format!("encode hello: {e}")))?;
            write_frame(&mut send, &hello_bytes).await?;

            let ack_bytes = read_frame(&mut recv).await?;
            let ack: HostAck = serde_json::from_slice(&ack_bytes)
                .map_err(|e| RoamingError::Transport(format!("decode ack: {e}")))?;
            Ok::<_, RoamingError>((send, recv, ack))
        };
        let (send, recv, ack) = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake)
            .await
            .map_err(|_| RoamingError::Transport("handshake timed out".into()))??;

        match ack {
            HostAck::Accepted { agent_id } => {
                // Host-controlled display text: printed by connect/delegate
                // and persisted for `roam connections`. Sanitize like client
                // labels so a malicious host can't inject terminal control
                // sequences.
                let agent_id =
                    sanitize_label(agent_id).unwrap_or_else(|| "remote-agent".to_string());
                self.directory
                    .record_connect(
                        conn.remote_id(),
                        None,
                        Direction::Outbound,
                        Some(agent_id.clone()),
                        now_ms(),
                    )
                    .await;
                // Balance the connect when the dialed connection closes for
                // any reason (normal command teardown included), so the
                // directory doesn't report the remote connected forever.
                {
                    let directory = self.directory.clone();
                    let remote = conn.remote_id();
                    let watched = conn.clone();
                    tokio::spawn(async move {
                        watched.closed().await;
                        directory.record_disconnect(remote, now_ms()).await;
                    });
                }
                Ok(RoamingClientStream {
                    agent_id,
                    conn,
                    send,
                    recv,
                })
            }
            HostAck::Rejected { code } => Err(RoamingError::Rejected(code)),
        }
    }
}

/// A dialed, authorized client stream to a remote agent.
pub struct RoamingClientStream {
    pub agent_id: String,
    /// Kept alive so the connection isn't dropped while the stream is in use.
    pub conn: Connection,
    pub send: iroh::endpoint::SendStream,
    pub recv: iroh::endpoint::RecvStream,
}

impl RoamingClientStream {
    /// Consume the stream into `futures::io` read/write halves ready to feed to
    /// an ACP client transport (e.g. `ByteStreams::new(send, recv)`), plus the
    /// live [`Connection`] which the caller must keep alive for the duration of
    /// the session. This saves consumers from repeating the tokio-compat dance.
    pub fn into_futures_io(
        self,
    ) -> (
        impl AsyncWrite + Send + Unpin,
        impl AsyncRead + Send + Unpin,
        Connection,
    ) {
        (self.send.compat_write(), self.recv.compat(), self.conn)
    }
}

struct RoamingAcpHandler {
    node: Arc<RoamingNode>,
    server: Arc<dyn AcpStreamServer>,
}

impl std::fmt::Debug for RoamingAcpHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RoamingAcpHandler")
    }
}

impl ProtocolHandler for RoamingAcpHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let client = connection.remote_id();

        // Bound the whole handshake phase so a peer that connects and then
        // stalls (never opening its stream or never sending a hello) is dropped
        // instead of parking this task forever (Slowloris guard).
        let handshake = async {
            let (send, mut recv) = connection.accept_bi().await?;
            // Authorize the key *before* acking so "Accepted" is truthful.
            let decision = self.authorize(client, &mut recv).await;
            Ok::<_, AcceptError>((send, recv, decision))
        };
        let (mut send, recv, decision) =
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake).await {
                Ok(Ok(parts)) => parts,
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    tracing::info!(%client, "roaming: handshake timed out; dropping connection");
                    return Ok(());
                }
            };
        match decision {
            Ok(label) => {
                let agent_id = self.server.agent_id();
                let ack = HostAck::Accepted {
                    agent_id: agent_id.clone(),
                };
                if let Err(e) = send_ack(&mut send, &ack).await {
                    tracing::warn!("roaming: failed to send accept ack: {e}");
                    return Ok(());
                }
                self.node.register_live(client, connection.clone()).await;

                // Close the accept/register TOCTOU: a revocation that lands
                // after `authorize` read the allowlist but before the line
                // above can be processed by the revocation watcher while this
                // connection is still absent from `live`, so it would never be
                // force-closed. Re-read trust now that we are registered; if
                // the key is no longer allowed, drop the connection here. A
                // failed reload fails closed, matching `authorize`.
                let still_allowed = match &self.node.trust_path {
                    Some(path) => match TrustBook::load(path) {
                        Ok(book) => book.is_allowed(&client),
                        Err(e) => {
                            tracing::warn!(%client, "roaming: trust recheck failed, closing: {e}");
                            false
                        }
                    },
                    None => self.node.trust.lock().await.is_allowed(&client),
                };
                if !still_allowed {
                    self.node.unregister_live(client, &connection).await;
                    connection.close(0u32.into(), b"revoked");
                    tracing::info!(%client, "roaming: revoked during accept; closed connection");
                    return Ok(());
                }

                self.node
                    .directory
                    .record_connect(client, label, Direction::Inbound, Some(agent_id), now_ms())
                    .await;
                let recv_box: Box<dyn AsyncRead + Send + Unpin> = Box::new(recv.compat());
                let send_box: Box<dyn AsyncWrite + Send + Unpin> = Box::new(send.compat_write());
                if let Err(e) = self.server.serve_stream(client, recv_box, send_box).await {
                    tracing::warn!("roaming: ACP session ended with error: {e}");
                }
                self.node.unregister_live(client, &connection).await;
                self.node
                    .directory
                    .record_disconnect(client, now_ms())
                    .await;
            }
            Err(reason) => {
                let ack = HostAck::Rejected {
                    code: reason.to_string(),
                };
                let _ = send_ack(&mut send, &ack).await;
                // Returning drops the connection, and a QUIC close can beat
                // the ack to the peer — which would turn a precise "rejected:
                // not_allowlisted" into a generic "connection lost" on the
                // client. Finish the stream and give the client a moment to
                // read the ack and close first.
                let _ = send.finish();
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(3), connection.closed())
                        .await;
                tracing::info!(%client, reason = %reason, "roaming: rejected connection");
            }
        }
        Ok(())
    }
}

impl RoamingAcpHandler {
    /// Authorize a connection purely by the transport-authenticated peer key.
    /// The peer's identity is already proven by QUIC-TLS; this only checks the
    /// allowlist. The [`ClientHello`] carries just a display label — nothing
    /// trusted for authorization. Returns the sanitized label on success.
    async fn authorize(
        &self,
        client: EndpointId,
        recv: &mut iroh::endpoint::RecvStream,
    ) -> Result<Option<String>, String> {
        let hello_bytes = read_frame(recv).await.map_err(|e| e.to_string())?;
        let hello: ClientHello =
            serde_json::from_slice(&hello_bytes).map_err(|e| format!("bad hello: {e}"))?;

        // Re-read the persisted allowlist so `peers accept`/`revoke` from a
        // separate process take effect against this running share without a
        // restart. Reads are atomic (writers rename into place), so we never see
        // a half-written file. If a path is set but the read genuinely fails we
        // fail *closed* rather than fall back to a stale in-memory book — a
        // just-revoked peer must not slip through. We snapshot under the lock and
        // drop it before any decision so the mutex isn't held across I/O.
        let refreshed = match &self.node.trust_path {
            Some(path) => match TrustBook::load(path) {
                Ok(book) => Some(book),
                Err(e) => {
                    tracing::warn!(%client, "roaming: trust reload failed, refusing: {e}");
                    return Err("unavailable".to_string());
                }
            },
            None => None,
        };
        let trust = match &refreshed {
            Some(book) => book,
            None => &*self.node.trust.lock().await,
        };
        if trust.is_key_revoked(&client) {
            return Err("revoked".to_string());
        }
        if !trust.is_allowed(&client) {
            return Err("not_allowlisted".to_string());
        }

        Ok(hello.label.and_then(sanitize_label))
    }
}

async fn send_ack(
    send: &mut iroh::endpoint::SendStream,
    ack: &HostAck,
) -> Result<(), RoamingError> {
    let bytes =
        serde_json::to_vec(ack).map_err(|e| RoamingError::Transport(format!("encode ack: {e}")))?;
    write_frame(send, &bytes).await
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The label is attacker-controlled display text (it comes from the connecting
/// peer's hello) and is surfaced in `roam connections`. Strip control
/// characters and cap the length so it can't corrupt terminal output; drop it
/// entirely if nothing printable remains.
fn sanitize_label(label: String) -> Option<String> {
    const MAX_LABEL_CHARS: usize = 64;
    let cleaned: String = label
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LABEL_CHARS)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_label;

    #[test]
    fn sanitize_label_strips_control_chars_and_caps_length() {
        assert_eq!(sanitize_label("laptop".into()), Some("laptop".into()));
        // Control chars (incl. ANSI escape) are removed.
        assert_eq!(
            sanitize_label("lap\x1b[31mtop\n".into()),
            Some("lap[31mtop".into())
        );
        // Whitespace-only / empty collapses to None.
        assert_eq!(sanitize_label("   ".into()), None);
        assert_eq!(sanitize_label("\n\t".into()), None);
        // Length is capped.
        let long = "x".repeat(200);
        assert_eq!(sanitize_label(long).unwrap().chars().count(), 64);
    }
}
