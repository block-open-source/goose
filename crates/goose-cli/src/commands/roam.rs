//! `goose roam` — peer-to-peer agent access over iroh.
//!
//! The model is deliberately infrastructural: roaming is just an authenticated
//! p2p ACP transport. Each node has one identity and produces a **connection
//! card** (`roam id`) — a non-secret string carrying its public key and how to
//! reach it. You swap cards with another node and each side chooses to **accept**
//! the other's key. A connection only succeeds when the host has accepted the
//! dialer's key; there is no bearer token that grants access by possession. An
//! accepted peer gets goose's full ACP surface.
//!
//! Subcommands:
//! * `id` — print this node's connection card (share it with a peer).
//! * `share` — serve this node's agent to accepted peers over ACP.
//! * `peers` — manage saved peer cards and which keys you accept.
//! * `connect` / `delegate` / `bridge` — reach a peer that has accepted you.
//! * `connections` (alias `list`) — show live/observed connections.

use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Subcommand;
use goose::acp::server::AcpBuiltinSelection;
use goose::acp::server_factory::{AcpServer, AcpServerFactoryConfig};
use goose::agents::GoosePlatform;
use goose::config::paths::Paths;
use goose::config::{Config, ConfigError};
use goose_roaming::{
    default_key_path, parse_endpoint_id, ConnectionCard, Directory, EndpointId, RelayEntry,
    RelaySettings, RoamingConfig, RoamingIdentity, RoamingNode, TrustBook,
};

use crate::commands::roam_full_bridge::FullAcpBridge;

const CARD_SCHEME: &str = "goose+roam://";

pub(crate) fn directory_path() -> std::path::PathBuf {
    Paths::state_dir().join("roaming_directory.json")
}

pub(crate) fn trust_path() -> std::path::PathBuf {
    Paths::config_dir().join("roaming_trust.json")
}

fn peerbook_path() -> std::path::PathBuf {
    Paths::config_dir().join("roaming_peers.json")
}

/// Config key (or `GOOSE_ROAM_RELAYS` env) overriding the relay URLs the
/// roaming endpoint uses. When unset, roaming uses the managed relays below —
/// never iroh's shared public n0 relays.
const CONFIG_ROAM_RELAYS_KEY: &str = "GOOSE_ROAM_RELAYS";
/// Optional shared bearer token (secret) presented to every *explicitly
/// configured* relay (for gated / `AccessConfig::Restricted` relays). Not
/// applied to the default managed relays, which register open.
const CONFIG_ROAM_RELAY_TOKEN_KEY: &str = "GOOSE_ROAM_RELAY_TOKEN";

/// Default managed iroh relays — the same four dedicated relays mesh-llm uses
/// (provisioned via services.iroh.computer on the `iroh.link` domain), one per
/// region for global coverage. They register open (`AccessConfig::Everyone`, no
/// auth token), so a browser client can reach them over `wss://` with no auth
/// header. We default to these rather than iroh's public n0 relays so roaming
/// never depends on the shared, rate-limited, no-SLA public relays.
///
/// Override with `GOOSE_ROAM_RELAYS` to point at other relays (e.g. a Block-run
/// deployment).
const DEFAULT_ROAM_RELAYS: &[&str] = &[
    "https://usw1-2.relay.michaelneale.mesh-llm.iroh.link./", // US West
    "https://use1-1.relay.michaelneale.mesh-llm.iroh.link./", // US East
    "https://euc1-1.relay.michaelneale.mesh-llm.iroh.link./", // EU Central
    "https://aps1-1.relay.michaelneale.mesh-llm.iroh.link./", // Asia-Pacific South
];

/// Resolve the relay settings for a roaming endpoint.
///
/// Uses `GOOSE_ROAM_RELAYS` (env or config file) when set — optionally
/// authenticated with the `GOOSE_ROAM_RELAY_TOKEN` secret applied to each —
/// otherwise the default managed relays. Never iroh's public n0 relays.
///
/// Fails when `GOOSE_ROAM_RELAYS` is set but unreadable: a deployment that
/// configured private relays must not silently fall back to the managed ones.
pub(crate) fn resolve_relay_settings() -> Result<RelaySettings> {
    let config = Config::global();
    let urls: Vec<String> = match config.get_param::<Vec<String>>(CONFIG_ROAM_RELAYS_KEY) {
        Ok(urls) => urls,
        Err(ConfigError::NotFound(_)) => Vec::new(),
        Err(error) => {
            return Err(anyhow::anyhow!(
                "{CONFIG_ROAM_RELAYS_KEY} is set but could not be read as a list of relay \
                 URLs; refusing to fall back to the default relays: {error}"
            ))
        }
    };
    // Only explicit relay URLs use a token; skip the secret-store read (and
    // any keyring stall it may involve) entirely on the default-relay path.
    if urls.iter().all(|u| u.trim().is_empty()) {
        return Ok(build_relay_settings(urls, None));
    }
    let token = match config.get_secret::<String>(CONFIG_ROAM_RELAY_TOKEN_KEY) {
        Ok(token) => Some(token),
        Err(ConfigError::NotFound(_)) => None,
        // A configured token that cannot be read (keyring timeout, corrupt
        // store) must not silently become unauthenticated relay dials.
        Err(error) => {
            return Err(anyhow::anyhow!(
                "{CONFIG_ROAM_RELAY_TOKEN_KEY} could not be read; refusing to contact \
                 relays without the configured token: {error}"
            ))
        }
    };
    Ok(build_relay_settings(urls, token))
}

/// Pure relay-settings builder. With no configured URLs, use the default
/// managed relays (open, no token). With configured URLs (empty entries
/// filtered out), use those, applying a non-empty token to each.
fn build_relay_settings(urls: Vec<String>, token: Option<String>) -> RelaySettings {
    let urls: Vec<String> = urls.into_iter().filter(|u| !u.trim().is_empty()).collect();
    if urls.is_empty() {
        let entries = DEFAULT_ROAM_RELAYS
            .iter()
            .map(|url| RelayEntry::new(*url))
            .collect();
        return RelaySettings::Custom(entries);
    }
    let token = token.filter(|t| !t.is_empty());
    let entries = urls
        .into_iter()
        .map(|url| match &token {
            Some(token) => RelayEntry::with_auth(url, token.clone()),
            None => RelayEntry::new(url),
        })
        .collect();
    RelaySettings::Custom(entries)
}

#[derive(Debug, Subcommand)]
pub enum RoamCommand {
    /// Print this node's connection card — the non-secret string you share with
    /// a peer so it can find and identify this node. Nothing in it is a secret;
    /// a peer must still be accepted (`roam peers accept`) before it can connect.
    #[command(visible_alias = "card")]
    Id {
        /// Also render the card as a QR code in the terminal (scan from a phone).
        #[arg(long)]
        qr: bool,
    },

    /// Pair a new device interactively: shows this node's card as a QR code,
    /// then reads the device's card from stdin and accepts it in one step
    /// (the equivalent of `roam peers add` + `roam peers accept`).
    Pair {
        /// Nickname to save the device under (defaults to `device-<short id>`).
        #[arg(long)]
        name: Option<String>,
    },

    /// Serve this node's agent to accepted peers over ACP.
    ///
    /// Only peers whose key you have accepted (`roam peers accept`) can connect.
    /// Each connected peer gets goose's full ACP surface — it drives its own
    /// sessions (new/list/load/prompt) backed by this node's session store.
    Share {
        /// Builtin extensions to load into the hosted agent.
        #[arg(long = "with-builtin", value_delimiter = ',')]
        builtins: Vec<String>,

        /// Working directory the hosted agent runs in. Defaults to the directory
        /// `roam share` was started in. The connecting client's own path is
        /// always ignored.
        #[arg(long)]
        cwd: Option<std::path::PathBuf>,

        /// Also render the connection card as a QR code in the terminal.
        #[arg(long)]
        qr: bool,
    },

    /// Open a quick interactive REPL against a remote agent (debug/peek).
    ///
    /// This is a minimal built-in chat loop, handy for a quick sanity check. For
    /// real work, prefer `bridge` (drive the remote agent from Zed or any other
    /// ACP client) or `delegate` (scriptable one-shot tasks).
    Connect {
        /// A saved peer nickname (see `roam peers`) or a `goose+roam://...` card.
        target: String,

        /// Optional label reported to the host's directory.
        #[arg(long)]
        label: Option<String>,
    },

    /// Delegate a one-shot task to a remote agent and print its response.
    ///
    /// This is a thin ACP client: it connects, opens a session (new, or the one
    /// named by `--session`), sends the task as a prompt, prints the reply, and
    /// exits. Session enumeration and resume are plain ACP (`session/list` /
    /// `session/load`) served by the remote's full ACP surface.
    Delegate {
        /// A saved peer nickname (see `roam peers`) or a `goose+roam://...` card.
        target: String,
        /// The task/question to send to the remote agent. Omit when using
        /// `--list-sessions`.
        task: Option<String>,
        /// Run the task against an existing remote session id (via `session/load`)
        /// instead of a fresh session. List ids with `--list-sessions`.
        #[arg(long, value_name = "SESSION_ID")]
        session: Option<String>,
        /// List the remote agent's sessions (`session/list`) and exit.
        #[arg(long)]
        list_sessions: bool,
    },

    /// Expose a remote agent as a local ACP endpoint that any ACP client can drive.
    ///
    /// Unlike `connect` (which has its own terminal UI), `bridge` runs no UI and
    /// no agent: it transparently proxies ACP between a local transport and the
    /// remote agent. Point Zed or any other ACP client at it
    /// and the remote agent behaves as if it were running locally.
    ///
    /// Defaults to stdio (for a client that spawns `goose roam bridge ...` as a
    /// subprocess). Use `--listen` to accept a single TCP connection instead.
    Bridge {
        /// A saved peer nickname (see `roam peers`) or a `goose+roam://...` card.
        target: String,

        /// Listen for one ACP client on this TCP address (e.g. `127.0.0.1:8900`)
        /// instead of using stdio. Loopback only: the TCP side carries the
        /// remote agent's full ACP surface with no authentication of its own.
        #[arg(long, value_name = "ADDR")]
        listen: Option<String>,

        /// Allow `--listen` on a non-loopback address. Anyone who can reach
        /// the socket gets the remote agent — put real authentication or a
        /// private network in front of it.
        #[arg(long, requires = "listen")]
        allow_remote_clients: bool,

        /// Optional label reported to the host's directory.
        #[arg(long)]
        label: Option<String>,
    },

    /// Manage saved peer cards and which peer keys this node accepts.
    Peers {
        #[command(subcommand)]
        command: Option<PeersCommand>,
    },

    /// Show live/observed connections to and from this node.
    #[command(visible_alias = "list")]
    Connections,
}

#[derive(Debug, Subcommand)]
pub enum PeersCommand {
    /// Save a peer's connection card to the address book so you can reach it by
    /// name. Does NOT let them connect to you — use `accept` for that.
    Add {
        /// The peer's `goose+roam://...` card.
        card: String,
        /// Friendly nickname (defaults to a short id if omitted).
        name: Option<String>,
    },
    /// Accept inbound connections from a peer's key. The target is a saved
    /// nickname or a `goose+roam://...` card (which is also saved to the address
    /// book). An accepted peer gets goose's full ACP surface.
    Accept {
        /// A saved nickname or a `goose+roam://...` card.
        target: String,
        /// Nickname to save an inline card under (defaults to a short id).
        /// Ignored when the target is already a saved nickname.
        name: Option<String>,
    },
    /// Stop accepting a peer: a saved nickname, a card, or a raw endpoint id.
    /// A running share force-closes the peer's live connections within seconds.
    Revoke { target: String },
    /// Remove a saved peer from the address book (does not change acceptance).
    Remove { name: String },
    /// Rename a saved peer.
    Rename { from: String, to: String },
    /// List saved peers and which keys are accepted (default).
    List,
}

pub async fn handle_roam_command(command: RoamCommand) -> Result<()> {
    match command {
        RoamCommand::Id { qr } => handle_id(qr).await,
        RoamCommand::Pair { name } => handle_pair(name).await,
        RoamCommand::Share { builtins, cwd, qr } => handle_share(builtins, cwd, qr).await,
        RoamCommand::Connect { target, label } => handle_connect(target, label).await,
        RoamCommand::Delegate {
            target,
            task,
            session,
            list_sessions,
        } => handle_delegate(target, task, session, list_sessions).await,
        RoamCommand::Bridge {
            target,
            listen,
            allow_remote_clients,
            label,
        } => handle_bridge(target, listen, allow_remote_clients, label).await,
        RoamCommand::Peers { command } => handle_peers(command.unwrap_or(PeersCommand::List)).await,
        RoamCommand::Connections => handle_list().await,
    }
}

/// Bind a node briefly to read its live card (id + relay URLs), waiting for a
/// relay so the card carries a reachable address.
/// Render a card as a terminal QR code (unicode half-blocks) to stderr.
fn print_qr(card: &str) {
    match qrcode::QrCode::new(card.as_bytes()) {
        Ok(code) => {
            let art = code
                .render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .build();
            eprintln!("{art}");
            eprintln!("scan with a phone camera, then paste the decoded card into your client");
        }
        Err(err) => eprintln!("could not render QR: {err}"),
    }
}

async fn handle_id(qr: bool) -> Result<()> {
    let identity = load_identity()?;
    let node = RoamingNode::bind(RoamingConfig {
        identity,
        relay: resolve_relay_settings()?,
        trust: TrustBook::new(),
        trust_path: None,
        directory: Directory::new(),
        bind_addr: None,
        relay_tls: None,
    })
    .await?;
    eprintln!("contacting relay so the card carries a reachable address...");
    node.wait_online(std::time::Duration::from_secs(15)).await;
    let card = node.card();
    eprintln!("your connection card (share this with a peer):");
    println!("{}", card.encode()?);
    if qr {
        eprintln!();
        print_qr(&card.encode()?);
    }
    eprintln!();
    eprintln!("  endpoint id : {}", card.endpoint_id);
    eprintln!("  fingerprint : {}", card.fingerprint());
    eprintln!();
    eprintln!("the peer adds it with:  goose roam peers add '<card>' <name>");
    eprintln!("and accepts you with:   goose roam peers accept <name>");
    node.shutdown().await?;
    Ok(())
}

/// Interactive one-shot pairing: show this node's card + QR, read the device's
/// card from stdin, confirm its fingerprint, then save and accept it — the
/// same PeerBook/TrustBook writes as `peers add` + `peers accept`.
async fn handle_pair(name: Option<String>) -> Result<()> {
    let identity = load_identity()?;
    let node = RoamingNode::bind(RoamingConfig {
        identity,
        relay: resolve_relay_settings()?,
        trust: TrustBook::new(),
        trust_path: None,
        directory: Directory::new(),
        bind_addr: None,
        relay_tls: None,
    })
    .await?;
    eprintln!("contacting relay so the card carries a reachable address...");
    node.wait_online(std::time::Duration::from_secs(15)).await;
    let card = node.card();
    let encoded = card.encode()?;
    eprintln!("your connection card:");
    println!("{encoded}");
    eprintln!();
    print_qr(&encoded);
    eprintln!();
    eprintln!("  endpoint id : {}", card.endpoint_id);
    eprintln!("  fingerprint : {}", card.fingerprint());
    node.shutdown().await?;
    eprintln!();
    eprintln!("on the new device: scan the QR with its client (or paste the card),");
    eprintln!("then copy the card from its pairing screen back here.");
    eprintln!();

    eprint!("paste the device's card (from its pairing screen): ");
    let device_card = read_stdin_line()?;
    let device_card = device_card.trim();
    if device_card.is_empty() {
        anyhow::bail!("no card entered; pairing cancelled");
    }
    let decoded = ConnectionCard::decode(device_card)
        .context("that does not look like a goose+roam:// connection card")?;
    let name =
        name.unwrap_or_else(|| format!("device-{}", short_id(&decoded.endpoint_id.to_string())));

    eprintln!();
    eprintln!("  endpoint id : {}", decoded.endpoint_id);
    eprintln!("  fingerprint : {}", decoded.fingerprint());
    eprintln!("verify the fingerprint matches the one shown on the device.");
    eprint!("accept this device as `{name}`? [y/N] ");
    let answer = read_stdin_line()?;
    if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
        eprintln!("pairing cancelled; nothing was saved");
        return Ok(());
    }

    goose_roaming::PeerBook::update(peerbook_path(), |book| {
        book.save(&name, device_card, now_ms())
    })?;
    let path = trust_path();
    TrustBook::update(&path, |trust| trust.accept(&decoded.endpoint_id)).with_context(|| {
        format!(
            "trust file {} is unreadable or corrupt; refusing to modify it",
            path.display()
        )
    })?;

    eprintln!("saved and accepted `{name}` ({})", decoded.endpoint_id);
    eprintln!("done — the device can now connect to any share/serve on this machine");
    Ok(())
}

fn read_stdin_line() -> Result<String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("failed to read from stdin")?;
    Ok(line)
}

async fn handle_peers(command: PeersCommand) -> Result<()> {
    let book = goose_roaming::PeerBook::load(peerbook_path())?;
    match command {
        PeersCommand::Add { card, name } => {
            let decoded = ConnectionCard::decode(&card)?;
            let name = name.unwrap_or_else(|| short_id(&decoded.endpoint_id.to_string()));
            goose_roaming::PeerBook::update(peerbook_path(), |book| {
                book.save(&name, &card, now_ms())
            })?;
            eprintln!(
                "saved peer `{name}` -> {} (fingerprint {})",
                decoded.endpoint_id,
                decoded.fingerprint()
            );
            eprintln!("accept connections from it with: goose roam peers accept {name}");
            Ok(())
        }
        PeersCommand::Accept { target, name } => {
            // Resolve to a card: a saved name, or an inline card we also save.
            let card = match ConnectionCard::decode(&target) {
                Ok(card) => {
                    let name = name.unwrap_or_else(|| short_id(&card.endpoint_id.to_string()));
                    goose_roaming::PeerBook::update(peerbook_path(), |book| {
                        book.save(&name, &target, now_ms())
                    })?;
                    card
                }
                Err(_) => {
                    if name.is_some() {
                        eprintln!("note: `{target}` is a saved peer; ignoring the extra name arg");
                    }
                    let rec = book.get(&target).ok_or_else(|| {
                        anyhow::anyhow!(
                            "no saved peer `{target}` and it is not a card; add it first with \
                             `goose roam peers add`"
                        )
                    })?;
                    rec.card.clone()
                }
            };
            let path = trust_path();
            TrustBook::update(&path, |trust| trust.accept(&card.endpoint_id)).with_context(
                || {
                    format!(
                        "trust file {} is unreadable or corrupt; refusing to modify it",
                        path.display()
                    )
                },
            )?;
            eprintln!("accepting connections from {}", card.endpoint_id);
            eprintln!("verify the fingerprint out of band: {}", card.fingerprint());
            eprintln!("a running `goose roam share` picks this up on the next connection");
            Ok(())
        }
        PeersCommand::Revoke { target } => {
            let key = resolve_key(&book, &target)?;
            let path = trust_path();
            TrustBook::update(&path, |trust| trust.revoke_key(&key)).with_context(|| {
                format!(
                    "trust file {} is unreadable or corrupt; refusing to modify it",
                    path.display()
                )
            })?;
            eprintln!("revoked {key}; it can no longer connect");
            eprintln!("a running share also force-closes its live connections within seconds");
            Ok(())
        }
        PeersCommand::Remove { name } => {
            let existed =
                goose_roaming::PeerBook::update(peerbook_path(), |book| book.remove(&name))?;
            if existed {
                eprintln!("removed peer `{name}` from the address book");
            } else {
                eprintln!("no peer named `{name}`");
            }
            Ok(())
        }
        PeersCommand::Rename { from, to } => {
            goose_roaming::PeerBook::update(peerbook_path(), |book| book.rename(&from, &to))?;
            eprintln!("renamed `{from}` -> `{to}`");
            Ok(())
        }
        PeersCommand::List => {
            let trust =
                TrustBook::load(&trust_path()).context("trust file is unreadable or corrupt")?;
            let accepted: std::collections::HashSet<String> =
                trust.allowed_keys().into_iter().collect();
            let peers = book.list();
            if peers.is_empty() && accepted.is_empty() {
                eprintln!("no saved peers; add one with `goose roam peers add '<card>' <name>`");
                return Ok(());
            }
            println!("{:<16} {:<8} ENDPOINT ID", "NAME", "ACCEPT");
            for p in &peers {
                let accept = if accepted.contains(&p.endpoint_id) {
                    "yes"
                } else {
                    "no"
                };
                println!("{:<16} {accept:<8} {}", p.name, p.endpoint_id);
            }
            // Accepted keys with no saved card (accepted by raw id).
            let known: std::collections::HashSet<String> =
                peers.iter().map(|p| p.endpoint_id.clone()).collect();
            for id in &accepted {
                if !known.contains(id) {
                    println!("{:<16} {:<8} {id}", "(unsaved)", "yes");
                }
            }
            Ok(())
        }
    }
}

/// Resolve a target (saved nickname, inline card, or raw endpoint id) to a key.
fn resolve_key(book: &goose_roaming::PeerBook, target: &str) -> Result<EndpointId> {
    if let Ok(card) = ConnectionCard::decode(target) {
        return Ok(card.endpoint_id);
    }
    if let Some(rec) = book.get(target) {
        return Ok(rec.card.endpoint_id);
    }
    parse_endpoint_id(target)
        .map_err(|_| anyhow::anyhow!("`{target}` is not a saved peer, a card, or an endpoint id"))
}

fn short_id(id: &str) -> String {
    id.chars().take(12).collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn handle_list() -> Result<()> {
    let entries = Directory::read_persisted(&directory_path());
    if entries.is_empty() {
        eprintln!("no roaming peers recorded yet");
        return Ok(());
    }
    println!("{:<10} {:<9} {:<20} ENDPOINT ID", "STATUS", "DIR", "AGENT");
    for e in entries {
        let status = if e.connected { "connected" } else { "seen" };
        let dir = match e.direction {
            goose_roaming::Direction::Inbound => "inbound",
            goose_roaming::Direction::Outbound => "outbound",
        };
        let agent = e.agent_id.unwrap_or_else(|| "-".to_string());
        let agent = if agent.chars().count() > 20 {
            let truncated: String = agent.chars().take(19).collect();
            format!("{truncated}…")
        } else {
            agent
        };
        println!("{status:<10} {dir:<9} {agent:<20} {}", e.endpoint_id);
    }
    Ok(())
}

/// This node's single long-lived identity. Its public key is what peers accept
/// and what the connection card advertises.
pub(crate) fn load_identity() -> Result<RoamingIdentity> {
    let path = default_key_path(&Paths::config_dir());
    RoamingIdentity::load_or_create(&path).context("failed to load roaming identity")
}

/// Roaming is an app-level service: every backend loads the same persisted
/// identity, so only one process may advertise the endpoint at a time. An OS
/// advisory lock decides ownership across all goose processes (desktop
/// windows, CLI serves, standalone shares); it auto-releases when the owner
/// dies — even on SIGKILL — so a standby can promote itself and paired
/// devices keep access.
/// `Ok(Some(file))` holds the lock; `Ok(None)` means another live process owns
/// the endpoint (standby is reasonable); `Err` is a real failure — unwritable
/// data dir, filesystem error — that must be surfaced, not retried silently.
pub(crate) fn try_acquire_roam_lock_owner() -> Result<Option<std::fs::File>> {
    use fs2::FileExt as _;
    use std::io::Write as _;

    let lock_path = Paths::data_dir().join("roam/serve.lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create roam lock dir {}", parent.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("cannot open roam lock file {}", lock_path.display()))?;
    if file.try_lock_exclusive().is_err() {
        return Ok(None);
    }
    file.set_len(0)?;
    writeln!(file, "{}", std::process::id())?;
    Ok(Some(file))
}

pub(crate) fn try_acquire_roam_lock() -> Result<std::fs::File> {
    try_acquire_roam_lock_owner()?.ok_or_else(|| {
        anyhow::anyhow!("another goose process is already running the roaming endpoint")
    })
}

async fn handle_share(
    builtins: Vec<String>,
    cwd: Option<std::path::PathBuf>,
    qr: bool,
) -> Result<()> {
    // Same app-level lock as `serve --roam`: both bind the one persisted
    // identity, and two processes advertising the same endpoint ID would race
    // for connections while hosting different working directories.
    let _roam_lock = try_acquire_roam_lock()?;
    let identity = load_identity()?;

    // The hosted agent runs in `--cwd` or the directory `roam share` was started
    // in; the connecting client's own path is meaningless here and is ignored.
    let session_cwd = match &cwd {
        Some(dir) => std::fs::canonicalize(dir)
            .with_context(|| format!("invalid --cwd: {}", dir.display()))?,
        None => std::env::current_dir().context("could not determine current directory")?,
    };
    // Fail here rather than advertising a share whose every session activation
    // would reject the host-imposed path.
    anyhow::ensure!(
        session_cwd.is_dir(),
        "--cwd must be a directory: {}",
        session_cwd.display()
    );

    // Load the accepted-peer allowlist. Peers are accepted out of band with
    // `roam peers accept`; this serve loop re-reads it per connection.
    let trust = TrustBook::load(&trust_path())
        .context("trust file is unreadable or corrupt; no peer can connect until it is fixed")?;
    let accepted_count = trust.allowed_keys().len();
    if accepted_count == 0 {
        eprintln!(
            "warning: no peers are accepted yet — no one can connect.\n\
             Accept a peer's key first: goose roam peers accept <name|card>"
        );
    }

    // The developer extension is on by default; explicitly requested builtins
    // are always loaded on top.
    let builtins = AcpBuiltinSelection {
        defaults: vec!["developer".to_string()],
        explicit: builtins,
    };

    let node = RoamingNode::bind(RoamingConfig {
        identity,
        relay: resolve_relay_settings()?,
        trust,
        // Re-read acceptance on each connection so `peers accept`/`revoke` from
        // another process take effect against this live share without restart.
        trust_path: Some(trust_path()),
        directory: Directory::persistent_owned(directory_path()),
        bind_addr: None,
        relay_tls: None,
    })
    .await?;

    let acp_server = Arc::new(AcpServer::new(AcpServerFactoryConfig {
        builtins,
        data_dir: Paths::data_dir(),
        config_dir: Paths::config_dir(),
        goose_platform: GoosePlatform::GooseCli,
        additional_source_roots: Vec::new(),
        session_cwd: Some(session_cwd.clone()),
        enable_scheduler: false,
    }));
    let agent_id = node.endpoint_id().to_string();
    let bridge = Arc::new(FullAcpBridge::new(
        acp_server,
        agent_id,
        session_cwd.clone(),
    ));
    node.share(bridge).await?;

    eprintln!("contacting relay...");
    if !node.wait_online(std::time::Duration::from_secs(15)).await {
        eprintln!("warning: endpoint did not come online; the card may lack a reachable address");
    }

    eprintln!("roaming agent is live");
    eprintln!("  endpoint id : {}", node.endpoint_id());
    eprintln!("  working dir : {}", session_cwd.display());
    eprintln!("  accepted    : {accepted_count} peer key(s)");
    eprintln!();
    eprintln!("your connection card (share with a peer so it can reach you):");
    println!("{}", node.card().encode()?);
    if qr {
        print_qr(&node.card().encode()?);
    }
    eprintln!();
    eprintln!("press Ctrl-C to stop sharing");

    tokio::signal::ctrl_c().await?;
    eprintln!("\nshutting down roaming endpoint...");
    node.shutdown().await?;
    Ok(())
}

/// Resolve a target (saved peer nickname or inline card) to a [`ConnectionCard`].
fn resolve_card(target: &str) -> Result<ConnectionCard> {
    if target.starts_with(CARD_SCHEME) {
        return ConnectionCard::decode(target).map_err(Into::into);
    }
    let book = goose_roaming::PeerBook::load(peerbook_path())?;
    match book.get(target) {
        Some(rec) => Ok(rec.card.clone()),
        None => anyhow::bail!(
            "no saved peer named `{target}` (and it is not a card); see `goose roam peers`"
        ),
    }
}

/// Bind this node and dial the target's card, returning the node + authorized
/// stream. The connection succeeds only if the remote has accepted this node's
/// key.
async fn dial_target(
    target: &str,
    label: Option<String>,
) -> Result<(
    std::sync::Arc<RoamingNode>,
    goose_roaming::RoamingClientStream,
)> {
    let card = resolve_card(target)?;
    let node = RoamingNode::bind(RoamingConfig {
        identity: load_identity()?,
        relay: resolve_relay_settings()?,
        trust: TrustBook::new(),
        trust_path: None,
        // Persist outbound observations so `roam connections` can show them.
        directory: Directory::persistent(directory_path()),
        bind_addr: None,
        relay_tls: None,
    })
    .await?;
    eprintln!("connecting to {}...", card.endpoint_id);
    let stream = node.connect(&card, label).await?;
    Ok((node, stream))
}

async fn handle_connect(target: String, label: Option<String>) -> Result<()> {
    let (node, stream) = dial_target(&target, label).await?;
    let agent_label = stream.agent_id.clone();
    eprintln!("connected to `{agent_label}`");
    let result = crate::commands::roam_client::run_interactive(stream, agent_label).await;
    node.shutdown().await?;
    result
}

async fn handle_delegate(
    target: String,
    task: Option<String>,
    session: Option<String>,
    list_sessions: bool,
) -> Result<()> {
    if list_sessions {
        let (node, stream) = dial_target(&target, Some("delegate".to_string())).await?;
        eprintln!("listing sessions on `{}`...", stream.agent_id);
        let result = crate::commands::roam_client::list_sessions(stream).await;
        node.shutdown().await?;
        let sessions = result?;
        if sessions.is_empty() {
            eprintln!("no sessions on the remote agent");
            return Ok(());
        }
        println!("{:<40} {:<20} UPDATED", "SESSION ID", "TITLE");
        for s in sessions {
            let title = s.title.unwrap_or_default();
            let title = if title.chars().count() > 20 {
                format!("{}…", title.chars().take(19).collect::<String>())
            } else {
                title
            };
            println!(
                "{:<40} {title:<20} {}",
                s.session_id,
                s.updated_at.unwrap_or_default()
            );
        }
        return Ok(());
    }

    let task = task.context("a task is required (or pass --list-sessions)")?;
    let (node, stream) = dial_target(&target, Some("delegate".to_string())).await?;
    match &session {
        Some(id) => eprintln!("delegating to `{}` session {id}...", stream.agent_id),
        None => eprintln!("delegating task to `{}`...", stream.agent_id),
    }
    let result = crate::commands::roam_client::delegate(stream, task, session).await;
    node.shutdown().await?;
    match result {
        Ok(response) => {
            println!("{response}");
            Ok(())
        }
        Err(e) => Err(e),
    }
}

async fn handle_bridge(
    target: String,
    listen: Option<String>,
    allow_remote_clients: bool,
    label: Option<String>,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    // Refuse a non-loopback --listen unless explicitly overridden: the TCP
    // side is an unauthenticated door to the remote agent's full ACP surface.
    if let Some(addr) = &listen {
        let parsed: std::net::SocketAddr = addr
            .parse()
            .with_context(|| format!("invalid --listen address `{addr}`"))?;
        if !parsed.ip().is_loopback() && !allow_remote_clients {
            anyhow::bail!(
                "--listen {addr} is not a loopback address; anyone who can reach it gets the \
                 remote agent with no authentication. Use 127.0.0.1/[::1], or pass \
                 --allow-remote-clients if you really mean to expose it."
            );
        }
    }

    let label = label.or_else(|| Some("bridge".to_string()));
    let (node, stream) = dial_target(&target, label).await?;
    let agent_id = stream.agent_id.clone();
    // The raw iroh streams carry post-handshake ACP and already implement
    // tokio's AsyncRead/AsyncWrite, so we splice them directly. `conn` must
    // outlive the splice.
    let goose_roaming::RoamingClientStream {
        conn,
        send: remote_send,
        recv: remote_recv,
        ..
    } = stream;

    let result = match listen {
        Some(addr) => {
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            let local = listener.local_addr()?;
            eprintln!("bridging remote agent `{agent_id}` on tcp://{local}");
            eprintln!("point an ACP client at this address; serving one connection");
            let (socket, peer) = listener.accept().await?;
            eprintln!("ACP client connected from {peer}");
            let (lr, lw) = socket.into_split();
            crate::commands::roam_proxy::splice(lr, lw, remote_send, remote_recv).await
        }
        None => {
            // stdio is a pure ACP transport: ONLY the splice may touch stdout.
            // All status goes to stderr so an ACP client reading stdout sees a
            // clean protocol stream.
            eprintln!("bridging remote agent `{agent_id}` over stdio; speak ACP on stdin/stdout");
            let stdin = tokio::io::stdin();
            let stdout = tokio::io::stdout();
            crate::commands::roam_proxy::splice(stdin, stdout, remote_send, remote_recv).await
        }
    };

    let _ = tokio::io::stderr().flush().await;
    drop(conn);
    node.shutdown().await?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_urls_uses_default_managed_relays_not_public() {
        match build_relay_settings(vec![], None) {
            RelaySettings::Custom(entries) => {
                assert_eq!(entries.len(), DEFAULT_ROAM_RELAYS.len());
                // Managed relays register open — no auth token attached.
                assert!(entries.iter().all(|e| e.auth_token.is_none()));
                assert!(entries.iter().all(|e| e.url.contains("iroh.link")));
            }
            other => panic!("expected Custom managed relays, got {other:?}"),
        }
    }

    #[test]
    fn blank_urls_are_filtered_and_fall_back_to_managed() {
        match build_relay_settings(vec!["  ".into(), "".into()], None) {
            RelaySettings::Custom(entries) => {
                assert_eq!(entries.len(), DEFAULT_ROAM_RELAYS.len());
            }
            other => panic!("expected Custom managed relays, got {other:?}"),
        }
    }

    #[test]
    fn custom_urls_without_token() {
        let settings = build_relay_settings(vec!["https://relay.example./".into()], None);
        match settings {
            RelaySettings::Custom(entries) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].url, "https://relay.example./");
                assert!(entries[0].auth_token.is_none());
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn custom_urls_apply_nonempty_token_to_each() {
        let settings = build_relay_settings(
            vec!["https://a.example./".into(), "https://b.example./".into()],
            Some("tok".into()),
        );
        match settings {
            RelaySettings::Custom(entries) => {
                assert_eq!(entries.len(), 2);
                assert!(entries
                    .iter()
                    .all(|e| e.auth_token.as_deref() == Some("tok")));
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn empty_token_is_ignored() {
        let settings =
            build_relay_settings(vec!["https://relay.example./".into()], Some(String::new()));
        match settings {
            RelaySettings::Custom(entries) => {
                assert!(entries[0].auth_token.is_none());
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }
}
