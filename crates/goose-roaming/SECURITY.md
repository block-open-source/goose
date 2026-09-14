# goose-roaming trust-model audit

A written audit of the card / allowlist trust model, done as a pre-undraft
gate. File references are to this crate unless noted.

## The model in one paragraph

Each node has one ed25519 keypair; the public key **is** its iroh
`EndpointId`, so identity is self-certifying — iroh's QUIC-TLS handshake
proves a peer holds the secret for the id it claims (`identity.rs`).
Authorization is a local, public-key allowlist (`trust.rs`): a host admits an
inbound connection only if the transport-authenticated key is on its
allowlist and not revoked. There is no bearer token, no capability string,
and nothing in a `ConnectionCard` grants access by possession.

## What an attacker gets at each position

| Position | What they can do |
|---|---|
| Holds a leaked card | Nothing. A card is public key + relay URLs; connecting with an unaccepted key is rejected before any ACP bytes flow (`node.rs` `authorize`). They can cause cheap handshake work (see DoS). |
| Controls a relay | Sees ciphertext and connection metadata (who talks to whom, when, volume). Cannot read or modify traffic — QUIC-TLS is end-to-end between the endpoints. Can drop/deny service. Relay auth tokens never travel in cards (`relay.rs`). |
| On-path network attacker | Same as a malicious relay: metadata + denial. Cannot impersonate either endpoint. |
| Steals a *device's* secret key | Full impersonation of that device. If the key was accepted by a host, they get that host's full ACP surface until revoked. This is the crown jewel; see key storage. |
| Local process as the host user | Can edit `roaming_trust.json` to accept any key — the same trust boundary as `~/.ssh/authorized_keys`. Not defended, by design: local user compromise is out of scope. |

## Control is one-way by construction

Although the transport is p2p, **control is never symmetric**. A node only
exposes an ACP surface by calling `RoamingNode::share()`, which is what
registers the `goose-acp/1` protocol handler — and the only callers are
`goose roam share` and `goose serve --roam`. Pure clients (the browser
webapp, `roam client`/`bridge`/`delegate`) bind an endpoint but never share:
they register no accept handler, so a host dialing back at them finds no
protocol to connect to. There is nothing to authorize or block — the surface
does not exist on the client side.

Relatedly, the allowlist gates **inbound only**: a host accepting a client's
key grants the client access to the host, and grants the host nothing in
return. "Mutual trust" in the docs means mutual *consent* (host authorizes
the key; client chooses whom to dial and verifies the fingerprint), not
mutual control — the same asymmetry as SSH's `authorized_keys` vs
`known_hosts`. Two machines that each want to drive the other run two
independent shares with two independent allowlists, so A→B without B→A is
the natural configuration, not a special mode.

## The blunt truth about authorization granularity

An accepted peer gets goose's **full ACP surface** with a fresh agent per
connection (`goose-cli/src/commands/roam_full_bridge.rs`), backed by the
host's session store, tools, and shell. **Accepting a key is equivalent to
granting shell access as the host user.** There is no per-peer capability
scoping, read-only mode, or session sandboxing. This is stated in the README
and must stay prominent in user-facing docs. Revocation force-closes live
connections within ~2s (`node.rs` `watch_revocations` / `enforce_trust`) but
cannot undo actions already taken.

Precise in-flight semantics of a force-close: the peer's ACP serving future
ends when its stream errors out, and any in-flight prompt turn is dropped at
its next await point — the agent loop stops mid-turn, within moments. The
residual is narrower than "work continues": an OS process a tool has
*already spawned* (e.g. a running shell command) is not `kill_on_drop` and
is only killed via the run's cancellation token, which a plain drop does not
fire — so an already-forked process may run to completion as an orphan. No
new work can start after the close.

## Findings by surface

### Identity & key storage
- Host/CLI: secret persisted as hex in a `0600` file, `0700` parent dir,
  atomic write (`identity.rs`). Plaintext on disk rather than OS keychain — a
  deliberate tradeoff (headless hosts, no keychain prompts); same posture as
  `~/.ssh/id_ed25519`.
- Browser: secret hex in `localStorage` (`web/webapp/src/main.tsx`). Weaker
  than the host: any XSS on the origin exfiltrates the key. Mitigations: the
  webapp has no third-party script; a stolen browser key grants access only
  to hosts that accepted it, and shows in `roam connections` / paired-devices
  UI where it can be revoked. Follow-up (nice-to-have): non-extractable
  WebCrypto keys are not usable here because iroh needs the raw ed25519 key;
  IndexedDB adds no secrecy over localStorage. Documented as a known
  limitation rather than fixed.

### Authentication
- Done entirely by iroh QUIC-TLS; `connection.remote_id()` is the
  authenticated key. The roaming handshake carries **no** credential — the
  `ClientHello` is just a display label (`handshake.rs`). Correct: nothing in
  the hello is trusted for authorization.
- Dialing: the client connects to the `EndpointAddr` derived from the card,
  and QUIC-TLS verifies the host presents the key matching that endpoint id.
  So the client's trust decision is "I trust the card I was handed" — which
  is why the out-of-band fingerprint exists.

### Authorization path (`node.rs` `authorize`)
- Allowlist is **re-read from disk on every inbound connection**, so
  accept/revoke from another process take effect on a running share without
  restart. Reads are atomic (writers temp+rename). ✅
- Trust reload failure **fails closed** ("unavailable"), never falls back to
  a stale in-memory book. ✅
- `revoked_keys` is checked before `allowed`, and `revoke_key` pins the key
  in the revoked set so a stale card can't silently re-add it; only an
  explicit `accept` clears a revocation. ✅
- Live revocation: `watch_revocations` polls the trust file mtime (~2s) and
  `enforce_trust` force-closes connections for keys no longer allowed,
  covered by two integration tests (`tests/end_to_end.rs`). ✅

### Handshake hardening
- Length-prefixed frames capped at 64 KiB (`frame.rs`) — a peer can't
  announce a huge frame. ✅
- Whole handshake bounded by a timeout (Slowloris guard) — a peer that
  connects and stalls is dropped. ✅
- Authorization happens **before** the ack, so `Accepted` is truthful. ✅
- The client-supplied label is sanitized (control chars stripped, 64-char
  cap) before it reaches terminal output (`sanitize_label`). ✅

### Connection card (`card.rs`)
- Non-secret by construction; safe to show as a QR code.
- Relay URLs from an untrusted card are constrained to `http(s)` schemes so a
  malicious card can't smuggle another scheme into the dialer. ✅
- Version-pinned decode; unknown versions rejected. ✅
- Fingerprint: first 128 bits of SHA-256 over the raw 32-byte endpoint key,
  displayed as eight 4-hex groups. 128 bits is second-preimage resistant
  against an attacker grinding keys to match a fingerprint a human compares
  out of band (the previous 48-bit form was not — fixed).
- Scope note: the fingerprint covers only the key, not the relay URLs. This
  is fine — relay URLs are untrusted reachability hints, and a tampered relay
  list yields at worst denial or metadata exposure, never impersonation.

### Trust state files
- `roaming_trust.json` / `roaming_peers.json` are `0644`: they contain only
  public keys and nicknames, nothing secret. Write access = user-level
  compromise (out of scope, above).
- `TrustBook::save` uses atomic temp+rename, so the per-connection reader and
  the revocation watcher never observe a torn file. ✅
- Read-modify-write is serialized: `TrustBook::update` holds a cross-process
  `fs2` advisory lock across load+mutate+save, so concurrent accept and revoke
  cannot lose an edit. All CLI accept/revoke/pair paths go through it. ✅

### `serve.json` (embedding)
- `0644` in the data dir, contains card / endpoint id / fingerprint — all
  non-secret. Removed on startup, written atomically once online. ✅

### DoS surface
- Anyone holding a card (or who learns the endpoint id) can connect and
  force TLS + one frame read; rejected peers never reach ACP, frames are
  capped, and the handshake is timeout-bounded. Rate limiting is delegated to
  the relay layer (managed relays are gated; n0 relays are rate-limited).
  Residual: accepted-peer resource exhaustion is untreated — an accepted peer
  is trusted, per the granularity note above.

## Non-goals / accepted gaps

- **No per-peer capability scoping** — accepted = full agent. Documented.
- **No per-peer action audit log** — the `Directory` records connections
  (who/when/direction), not what an accepted peer did. Session history is the
  audit trail, keyed to the host, not the peer.
- **No automatic key rotation / expiry** — keys live until revoked, like SSH.
- **No defense against the host user's own processes.**

## Verdict

The model is coherent and small: one keypair per node, transport-proven
identity, local allowlist, fail-closed reload, live revocation, non-secret
cards. The two things a reviewer must not lose sight of are (1) accept =
shell, and (2) the browser key sits in localStorage. Both are inherent to
the current design and are documented rather than mitigated.
