# goose-roaming

Peer-to-peer transport for goose agents, built on
[iroh](https://iroh.computer) (QUIC, using iroh's public relays for NAT
traversal).

It lets a goose agent accept connections from a remote ACP client (another
goose, or any other ACP client) that drives it, and lets a client
dial a remote agent to hold an interactive session, delegate a one-shot task, or
bridge it to a local ACP client — typically without port-forwarding.

This crate is a **standalone library** with no dependency on `goose` core (so
the iroh dependency stays out of core): it knows nothing about agents or
sessions, only identity, trust, and authenticated byte streams, and can be
embedded in any Rust application. The consumer surface is `RoamingNode`
(bind/share/connect), `RoamingConfig`, the two-method `AcpStreamServer` trait to
plug in whatever serves a stream, `TrustBook`, and `ConnectionCard` — see
`examples/echo_roundtrip.rs` for the whole flow in one file
(`cargo run -p goose-roaming --example echo_roundtrip`). The code that bridges
the transport to goose's agent machinery lives in `goose-cli` behind an optional
`roaming` feature; it isn't compiled unless that feature is enabled.

## The model: an authenticated ACP transport with mutual key trust

Roaming does one thing: provide an **authenticated peer-to-peer ACP transport**.
The host runs goose's real ACP server; the connecting side is an ACP client.
Everything "session-shaped" (list/load/new/prompt) is therefore plain ACP that
happens to run over a roaming connection — roaming adds no session semantics.

Trust is a **mutual, public-key allowlist** — WireGuard / SSH-known-hosts style,
not a capability token:

- Each node has **one** ed25519 identity that *is* its iroh `EndpointId`. The
  QUIC-TLS handshake proves a peer holds the secret for the id it claims, so a
  key cannot be impersonated. Persisted as hex in a `0600` file in the config dir.
- A node produces a **connection card** (`ConnectionCard`) — a non-secret string
  carrying its public key + relay URLs, plus a short fingerprint for out-of-band
  verification. It never expires and grants nothing on its own.
- You **swap cards** and each side **accepts** the other's key. A connection
  succeeds only if the host has accepted the dialer's key, and an accepted peer
  gets goose's full ACP surface. A leaked card lets no one in; there is no bearer
  token that works by possession.

## Concepts

- **`ConnectionCard`** — the shareable, non-secret identity + reachability string
  (`goose+roam://…`). Encodes public key + relay URLs; exposes `fingerprint()`.
- **`TrustBook`** — the local, mutual allowlist of accepted peer keys, plus
  revocations. Access exists *only* by accepting a key. Persisted atomically and
  re-read on each inbound connection, so `accept`/`revoke` take effect against a
  running `share` without a restart. Reload failure fails **closed**.
- **`Directory`** — an out-of-band record of connections that actually happened
  (inbound and outbound), built purely from observed connections. No gossip.
- **`PeerBook`** — a user-managed address book of remotes, by nickname; stores
  the peer's (non-secret) card.

## Flow

```
both:    bind endpoint ──▶ `roam id` prints a connection card ──▶ swap cards
host:    `roam peers accept <peer>` ──▶ `roam share` (serve to accepted keys)
client:  `roam peers add <card>` ──▶ dial via relay ──▶ handshake (label only)
host:    authorize by TLS-authenticated key ──▶ ACP serve() (full surface)
client:  run an ACP client over the same bi-stream
```

An iroh bidirectional stream is the byte transport for goose's existing
transport-agnostic ACP `serve` / `ByteStreams` seam, so hosting reuses the ACP
server and the client reuses the ACP client.

## CLI

Exposed via `goose roam` (in `goose-cli`, feature `roaming`):

| Command | Purpose |
|---|---|
| `roam id` (alias `card`) | Print this node's connection card |
| `roam peers add <card> [name]` | Save a peer's card to the address book |
| `roam peers accept <peer\|card> [name]` | Accept inbound connections from a key (names an inline card) |
| `roam peers revoke <peer\|card\|id>` | Stop accepting a key |
| `roam peers list` | Saved peers + which keys are accepted |
| `roam share [--cwd] [--with-builtin]` | Host this agent to accepted peers |
| `roam connect <peer\|card>` | Quick interactive REPL (debug/peek) |
| `roam delegate <peer\|card> ["<task>"] [--session <id>] [--list-sessions]` | One-shot task, or list/continue remote sessions |
| `roam bridge <peer\|card> [--listen <addr>]` | Expose the remote agent as a local ACP endpoint |
| `roam connections` | Live/observed connections (no gossip) |

## Testing across two disconnected machines

Build both with the roaming feature (`cargo build -p goose-cli --features
roaming`) — no shared network, VPN, or port-forwarding needed; the public n0
relays bridge them. On **each** machine run `goose roam id` and send the printed
`goose+roam://…` card to the other out of band (paste it in chat, etc.).

On **machine A** (the host): `goose roam peers accept '<B's card>'` then
`goose roam share` (optionally `--cwd <dir>`; it defaults to where `share`
started, and the connector's own path is always ignored). On **machine B**:
`goose roam peers add '<A's card>' boxA`, then either drive A interactively with
`goose roam connect boxA` (a prompt that runs on A's agent — its tools, files,
shell), hand it a one-shot task with `goose roam delegate boxA "what is 2+2?"`,
or `goose roam delegate boxA --list-sessions` / `--session <id> "<task>"` to
enumerate and continue A's sessions. Verify it's truly A doing the work by asking
something machine-specific (e.g. "what's your hostname and cwd?"). On the host,
`goose roam connections` shows who connected. If session creation hangs on
macOS, prefix with `GOOSE_DISABLE_KEYRING=1`.

## Design decisions & rationale

**Roaming is just an ACP transport.** The host runs the agent loop (its tools,
working directory, shell); the connecting side is an ACP client. Each connection
gets a fresh agent driving its own sessions (`FullAcpBridge` hands the stream to
goose's real `serve`). `connect` is a thin ACP client UI — not a provider
wrapper; wrapping the remote as a provider for a second local agent loop would
double the loop and defeat the point.

**The host controls the working directory.** ACP's `session/new` carries a cwd,
but the connector's absolute path is meaningless on the host machine. So the host
ignores the sent cwd and imposes its own (the directory `roam share` was started
in, or `--cwd`); the client sends only a placeholder.

**Trust is mutual and key-based, with no bearer path.** A card is non-secret and
grants nothing; a share admits no one until a key is explicitly accepted, so the
safe default (admit nobody) is the built-in one. Authorization uses the full
TLS-authenticated key; the handshake carries only a display label (not trusted).
Acceptance re-reads per connection (fail-closed) so revoke takes effect on a live
share.

**Acceptance is all-or-nothing.** An accepted peer gets goose's full ACP
surface (there is no per-request gate, so no finer-grained roles). Simultaneous
multi-viewer co-driving of one live session is a possible future feature; it is
not expressible over plain 1:1 ACP and is intentionally out of scope here.

**Delegation guardrails are about cost, not authorization.** The peer is already
trusted, so the concern with agent-to-agent delegation is runaway cost from loops
(A → B → A …). The `delegate` path auto-cancels tool-permission requests, since
there is no human present to answer them.

## What's deferred

- **Live multi-viewer co-driving** (paseo-style): several clients watching and
  steering *one* in-flight session at once. This isn't expressible over plain
  1:1 ACP — it needs a purpose-built multi-party session protocol (subscribe /
  snapshot / broadcast / steer with an explicit controller) layered over this
  transport. A future feature, deliberately not emulated via an ACP broker.
- Self-hosted relays (public n0 relays are rate-limited).

## Surfacing delegation to the model

The agent can reach other agents with **no new code**: a builtin skill
(`roam-delegate`) documents how to call `goose roam delegate <peer> "<task>"` via
the shell. It ships in core but is inert unless the `roaming` CLI feature is
built in, keeping iroh out of core.

## Browser web client

The **official browser client for roam** lives in a separate repo:
[aaif-goose/goose-mobile](https://github.com/aaif-goose/goose-mobile/tree/main/mobile-web)
(`mobile-web/`). It is a pure-browser React app that connects to a
`goose roam share` agent — iroh compiled to wasm runs *inside the browser tab*,
driving the agent over ACP. No Tauri, no local bridge; the tab is the roam peer.
The stock iroh wasm build tunnels QUIC over WebSocket to the relay (its UDP
transport is compiled out in browsers; a WebRTC custom transport could add
direct paths later).

It is fully decoupled from this crate: the `goose-roaming-web` wasm crate there
**mirrors** this crate's connection-card and frame wire format by copying its
constants (`CARD_VERSION`, `MAX_FRAME_BYTES`, card bounds). When you change the
wire format here, update goose-mobile in the same change — a drift will not fail
to compile there, it will break pairing at runtime.

## Prior art

Patterns here were informed by studying a sibling production project that runs
iroh 1.0 for distributed LLM inference: minimal-preset endpoints with custom
relay maps, ALPN-based stream dispatch, and reachability via relay-routing by
node id (a card needs only key + relay, not a fixed address).
