# Extension manager redesign

Status: active design and implementation tracker. The cleanup in step 1 shipped in
[#11645](https://github.com/aaif-goose/goose/pull/11645). The transport extraction in
step 2 is implemented in
[#12088](https://github.com/aaif-goose/goose/pull/12088) and awaiting review. The main
design is tracked in [#12084](https://github.com/aaif-goose/goose/issues/12084).

## Why

At the start of this work, `crates/goose/src/agents/extension_manager.rs` was 2,720
lines before the test module. It was a `Mutex<HashMap<String, Extension>>` plus
everything that had ever needed to touch it. Seven distinct jobs lived in one file:

1. Transport construction — process spawn, PATH resolution, stderr capture, docker
   exec, unix sockets, and the OAuth refresh / 401 / browser-fallback dance.
2. The registry of live clients, keyed by `name_to_key`.
3. Tool catalog assembly — fan-out `tools/list`, pagination, prefixing,
   `available_tools` filtering, schema normalization, dedupe, caching.
4. Tool name resolution, including recovery of names the model mangled.
5. Dispatch plumbing — notification stream merging, action-required stream
   registration, MCP-app resource hydration, `_meta` scrubbing.
6. Resource and prompt fan-out.
7. Leftovers: `get_planning_prompt` renders `plan.md` and has nothing to do with
   extensions; `search_available_extensions` formats a string about extensions that
   are *not* loaded.

Three consequences this work fixes:

**`add_extension` was the wrong shape.** A 280-line `match` under
`#[allow(clippy::too_many_lines)]` that interleaved config resolution, secret merging,
env substitution, docker-vs-local, malware checking, temp-file writing, transport
setup and registry insert. Every branch called something with 9-12 positional arguments
under `#[allow(clippy::too_many_arguments)]`. The same six values — `provider`,
`client_name`, `capabilities`, `working_dir`, `action_required`,
`Weak<ExtensionManager>` — travelled together everywhere. Step 2 made them a
`ConnectContext`.

**"Which extensions are in play" is expressed four incompatible ways.** The
`extension_name` filter argument, the `exclude` argument (added for code mode), the
`unprefixed_tools` flag and the `hidden` flag. Each is a special case bolted onto a
global catalog.

**There are two entry points to "enable an extension" and they disagree.**
`Agent::add_extension` fetches the session, passes `working_dir`, the container and the
session id, then persists session state. `ExtensionManagerClient::manage_extensions_impl`
upgrades the `Weak<ExtensionManager>` out of `PlatformExtensionContext` and calls
`add_extension(config, None, None, None)`. So when the *model* enables an extension it
starts in the process cwd instead of the session working dir, outside the docker
container and without `AGENT_SESSION_ID`. Both agent loops now persist successful model
changes after the tool call, but startup context and persistence still have different
owners. This is a live bug and it is a direct consequence of the layering.

## Protocol context

MCP 2026-07-28 changes the ground under this work, and the direction is favourable.
Verified against the spec, not assumed.

- The `initialize` / `initialized` handshake is retired. Each request carries its
  protocol version, client identity and capabilities in `_meta`. Capabilities are
  discovered on demand via `server/discover`.
- Multi Round-Trip Requests (MRTR) replace the server-initiated `elicitation/create`,
  `sampling/createMessage` and `roots/list` requests, which previously needed a
  held-open stream. A tool needing mid-call input returns `resultType: "input_required"`
  with `inputRequests`, and the client retries with `inputResponses` and `requestState`.
- Tool lists **MUST NOT** vary per-connection, but **MAY** vary by the authorization
  presented on the request, "since credentials are per-request input, not connection
  state." Deterministic ordering is recommended explicitly so clients can cache.
- `tools/list` responses carry `ttlMs` and `cacheScope`.
- `notifications/tools/list_changed` now requires the client to have opened a
  `subscriptions/listen` stream with `toolsListChanged: true`.
- Aggregating clients **SHOULD** disambiguate collisions by prefixing with a server
  identifier, and `serverInfo.name` is **not** guaranteed unique and **SHOULD NOT** be
  used for that.
- Clients **MUST** exclude tools whose `x-mcp-header` annotations violate the
  constraints, and **SHOULD** log why, "so that a single malformed tool definition does
  not prevent other valid tools from being used."

Sources: [tools specification](https://modelcontextprotocol.io/specification/draft/server/tools),
[2026-07-28 release notes](https://blog.modelcontextprotocol.io/posts/2026-07-28/).

Three consequences run through the rest of this document.

First, nothing should be built on `InitializeResult` — under the new protocol it does
not exist, and it never contained tools anyway.

Second, the two hard blockers to sharing a connection across sessions — per-connection
roots and server-to-client callback correlation — are **legacy-protocol constraints, not
permanent ones**. Under MRTR both travel per-request. Sharing moves from "maybe never"
to "the end state for 2026 servers, permanently off for legacy ones."

Third, `list_changed` invalidation now costs a held-open subscription stream, which is
in tension with not holding connections open for stateless HTTP. For those servers we
should lean on `ttlMs` rather than subscribing.

goose already negotiates 2026-07-28 with automatic fallback to 2025-11-25
(`ClientLifecycleMode::Auto` in `mcp_client.rs:704`), so both protocols are live at once.
The constraint on this redesign is not to bake the legacy `initialize` handshake into
anything durable, since half the servers we talk to never send one.

## Completed cleanup

[#11645](https://github.com/aaif-goose/goose/pull/11645) removed code that did not
belong in the redesign:

- `get_planning_prompt` was removed from the extension manager. The documented,
  user-editable `plan.md` template remains and is rendered by the agent.
- Frontend extensions and `FrontendToolRequest` were removed from configuration,
  runtime handling, providers and the UI.
- Inline Python extensions were removed.
- The obsolete `Sse` runtime variant was removed. Configuration loading still detects
  raw `type: sse` entries so users get the targeted migration warning.

We deliberately did not keep tombstone variants or add tolerant deserialization. These
features were experimental and effectively unused, so carrying compatibility machinery
would cost more than it protects. A session containing an old frontend or inline-Python
extension may fall back to the default extension set. A persisted conversation containing
`FrontendToolRequest` may fail to load. That compatibility break was accepted in
[#11642](https://github.com/aaif-goose/goose/issues/11642).

## Target design

### Declaration and lease are two types

The first draft called both of these `ExtensionSet` and claimed refcounting "falls out
for free." Both were wrong. A plain value cannot provide refcounted release, and an
`Arc` held by the host does not by itself bound idle lifetime.

```rust
struct ExtensionSet {                  // built at resolve time
    id: ExtensionSetId,                // opaque scope
    working_dir: PathBuf,              // from the session row
    execution_target: ExecutionTarget, // local, or the live container id
    extensions: Vec<ExtensionSelection>, // the only part that is persisted
}

struct ExtensionSelection {
    config: ExtensionConfig,
    available_tools: Vec<String>,      // per-selection, not per-connection
}

struct ExtensionScopeContext {         // runtime services, never persisted
    provider: SharedProvider,
    action_required: Arc<ActionRequiredManager>,
    scheduler: Option<Arc<dyn SchedulerTrait>>,
    client_profile: McpClientProfile,
}

struct ExtensionLease {                // resolved, live
    id: LeaseId,
    members: Vec<ExtensionHandle>,     // handles refer to slots, not clients
    catalog: ToolCatalog,
}
```

```rust
let lease = host.resolve(&extension_set, &scope_context).await?;
lease.tools().await?;
lease.call(name, params).await?;
```

`ExtensionSet` is inert data the agent builds each time it resolves. Only `extensions`
is persisted — that is `EnabledExtensionsState` today. `working_dir` comes from the
session row and `execution_target` is rebuilt from whatever container the session has
right now. It is the ephemeral Docker container id, and that is fine precisely because
it never hits disk. `ExtensionScopeContext` carries
the things extensions need in order to run but which must never hit disk — provider for
legacy sampling, action-required routing, scheduler, client name and host capabilities.
The first draft listed "client profile and capabilities" in the runtime key without
saying where they came from; this is where. For scoped legacy clients the slot captures
these services; for shared 2026 HTTP, request-specific services travel with the call.

`ExtensionSetId` is opaque. Sessions, subagents, scheduled executions and evaluations can
all be scopes without the host knowing what any of them mean.

`execution_target` closes a gap in the first draft, which named the container bug and
then dropped container identity from the struct. Two scopes differing only by container
must not share a runtime.

`available_tools` moves from `ExtensionConfig` onto `ExtensionSelection`. It is a
property of the selection, not the connection: two sets should share one server while
exposing different subsets. The host caches the raw published tool list; the lease
applies the filter.

`ExtensionSet::new` rejects duplicate normalized extension keys. Ordered precedence
handles duplicate *public tool names* across different extensions, but the same logical
extension appearing twice with different configuration is not a meaningful set.

### Who holds a lease, and for how long

- `Agent` owns the current lease.
- An inference borrows or snapshots it.
- After every tool call from that inference completes, a desired-state mutation may
  replace it: the agent resolves the new lease first, then drops the old one.
- Dropping a lease releases exactly that lease's selection references.

That is what makes the freeze rule concrete. It also means two leases for the same scope
overlap during every replacement, so slot bookkeeping is per lease, not per scope — a
`HashSet<ExtensionSetId>` would have the replacement's insert be a no-op and the old
lease's drop remove the scope while the replacement is still alive.

### Where the host lives

`ExtensionHost` must sit above `Agent`, at the application / execution-manager level.
One host per `Agent` cannot share runtimes across scopes or refcount them meaningfully,
which is most of the point. It owns slots, enforces the sharing policy, and is the only
thing permitted to start a process.

The hard part of moving it is **platform extensions**, whose `PlatformExtensionContext`
currently carries `SessionManager`, a session snapshot, the scheduler and a
`Weak<ExtensionManager>`. Platform extensions are in-process and session-aware; they can
never be `Shared`, and their context has to be rebuilt on top of
`ExtensionScopeContext`. Treat "platform extension context replacement" as explicit,
named work inside step 4, not as a detail.

### Slots, and three kinds of reference

The previous draft said a runtime may shut down only when leases and in-flight calls are
both zero. That contradicts the goal of making liveness invisible: a session that selects
Developer for its whole lifetime holds a lease forever, so after one call the process
would live until the session closes, and idle eviction would only ever help extensions
removed from every set.

Selection must not pin a process. Three distinct things:

- **Selection reference** — keeps the slot, its manifest and its configuration
  available.
- **In-flight reference** — keeps the *running process* alive.
- **Idle retention** — a running process with no in-flight calls may be stopped after a
  timeout even while still selected.

```rust
struct ExtensionSlot {
    manifest: Option<PublishedManifest>,
    runtime: RuntimeState,             // Stopped | Starting | Running | Failed
    selected_by: HashSet<LeaseId>,
    in_flight: usize,
    last_used: Option<Instant>,
}
```

A running process is eligible for idle shutdown whenever `in_flight == 0` and its idle
deadline has passed. Selection means the next call may restart it, not that it stays
resident.

This is why `ExtensionHandle` refers to a **slot**, not to a live client: a resolved
lease has to survive its runtime being stopped and restarted underneath it.

### The runtime key, initially

For step 4 to be genuinely behaviour-preserving, the key must include the scope id:

```
scope id
sanitized connection fingerprint
working directory
execution target (local / container identity)
client profile and capabilities
```

Today every `Agent` owns its own manager, so two sessions with the same config and
working directory already get separate processes. Keying only on
`(config, working_dir)` — as the first draft proposed — would start sharing them
silently, contradicting the claim that steps 1-4 change no behaviour. Scope id leaves the
key later, per transport, under an explicit policy.

`working_dir` is in the key because roots are per-connection in the legacy protocol:
`list_roots` returns a single working dir out of one `RwLock<PathBuf>`
(`crates/goose/src/agents/mcp_client.rs:373`). Two scopes with different working dirs
sharing a connection means one silently gets the wrong roots, and for the developer
extension that means reading the wrong tree. Under MRTR this goes away, but it binds
every server we talk to today.

Either way `update_working_dir`, which mutates that shared lock and fires
`roots/list_changed`, stops existing. Changing directory means presenting a set with a
different `working_dir`.

### The lease owns the catalog

Methods live on the resolved lease, not on a manager taking a set. `session_id` comes
off every signature — it is not a real dimension today, since there is one `Agent` per
ACP session (`crates/goose/src/acp/server.rs:1250`) and `McpClient` asserts it never
sees two session ids (`mcp_client.rs:247`).

- `tools()` — ordered concat, prefix, filter, dedupe over members
- `resolve(name)` and `call(name, params)`
- `list_resources()` / `read_resource(uri, ext)`
- `list_prompts()` / `get_prompt(ext, name, args)`
- `instructions()` — cached server metadata
- `moim()` — live per-inference context
- `configs()` / `names()` for persistence

`instructions()` and `moim()` stay separate. The first draft folded them into one
`prompt_contributions()`, which would conflate cached metadata with data that must be
recomputed every inference.

Resource and prompt listings return structured items carrying their owning extension.
Today `list_resources_from_extension` returns a preformatted `ContentBlock::text`
string, forcing every consumer to re-parse it or accept the one format.

Two things leave entirely:

- `get_ui_resources()` is `list_resources().filter(uri.starts_with("ui://"))`. Fold in.
- `search_available_extensions()` is about the config catalog — what *could* be
  enabled — not what is live. It belongs next to `config/extensions.rs` and the
  `manage_extensions` platform tool.

### One catalog, built once

Three namespaces exist today (server-side tool name, public `ext__tool` name,
normalized extension key) and each consumer re-derives the mapping differently:
`filter_tools` reads `goose_extension` meta but falls back to splitting on `"__"`;
`resolve_tool` is a loop with mangled-name recovery plus a fallback that dispatches to
an extension whose tool is not in the catalog at all; `get_tool_owner` reads meta.

Build `public_name -> (ext_key, actual_name, meta, resource_uri)` once. Prefixing lives
in one function. Filtering uses a stored key instead of string surgery. Resolution is a
lookup plus one recovery step. The spec endorses this shape directly — aggregating
clients SHOULD prefix with a server identifier — and warns that `serverInfo.name` is not
unique enough to be that identifier, which is why we key on our own normalized
extension key.

Catalog build is also where per-tool validation belongs: the spec requires clients to
exclude tools with invalid `x-mcp-header` annotations and log why, explicitly so one
malformed definition does not take out the rest. Today nothing does this.

Duplicate public names get deterministic precedence from the ordered set plus an explicit
diagnostic. Not a hard catalog error: two extensions exposing the same tool name is a
configuration some users are running today, and failing the whole catalog would break
them. Determinism removes the randomness; the diagnostic addresses the ambiguity.

### Caching splits from process lifetime

The cache is durable. The whole point of lazy start is that a fresh goose process can
build the first session's tool list and system prompt without spawning anything, which
an in-memory cache cannot deliver. Manifests live on disk in the data dir keyed by
`ManifestKey`; a slot holds an in-memory copy loaded from there, and a refresh writes
through.

Cache per section rather than as one blob — the protocol gives tools and discovery their
own freshness.

```rust
struct Cached<T> {
    value: T,
    expires_at: Option<SystemTime>,
    scope: CacheScope,
}

struct PublishedManifest {
    discovery: Cached<Discovery>,   // capabilities, server info, protocol version
    tools: Cached<Vec<Tool>>,
}
```

Prompts and resources stay uncached initially.

"Goose-owned type" means the envelope, not the tool definitions. `Tool` is rmcp's serde
form of the MCP wire object, and the wire object is the stable thing — more stable than
anything we would invent. So on disk an entry is a versioned goose envelope around MCP
wire JSON. An envelope version we do not recognise, or JSON that no longer parses, is a
cache miss, never an error.

Freshness policy by protocol:

- **2026 servers** — honour `ttlMs` and `cacheScope` from the response.
- **Legacy servers** — a configurable goose fallback TTL, with
  stale-while-revalidate.
- **Never unbounded.** "Only invalidated by a successful refresh or a fingerprint
  change" was wrong: a stopped `npx -y package` server cannot send
  `tools/list_changed`, and under 2026 that notification requires a held-open
  `subscriptions/listen` stream we may deliberately not be holding. Without a TTL a
  cache entry can stay stale forever.

Two keys, not one:

- **`ManifestKey`** — server identity, protocol and client profile, and **authorization
  identity**, since the spec permits published tools to vary by authorization.
- **`RuntimeKey`** — manifest identity plus the scope-dependent execution facts above.

`ManifestKey` is itself protocol-dependent. A 2026 server promises its tools do not vary
per connection, so one manifest is valid across working directories. A legacy server
promises nothing about whether roots, cwd, client capabilities or initialization context
affect what it publishes. So legacy manifests start scoped by the same facts as their
runtime, and only modern servers get the relaxed key.

"Hash of the resolved config" from the first draft is bad shorthand. Resolved config
contains secret *values*, which must not be hashed into a cache key; `description` and
`bundled` should not force a new runtime; and auth changes should. Define a sanitized
connection fingerprint explicitly.

Per-extension caching replaces today's single global `tools_cache`, invalidated wholesale
on add, remove and any extension's `tools/list_changed`, with an `AtomicU64` version
counter papering over the race. Invalidation becomes precise and the counter goes away.

A cached section is replaced only after a complete, successful, fully paginated refresh.
A partial refresh leaves the previous value intact.

### Lazy start

Serve the tool list and system prompt from the manifest cache; start the runtime on
first actual use. Whether a server is running becomes invisible to the caller. Today
`Agent::add_extensions_bulk` (`agent.rs:1371`) spawns every configured server before the
first token, whether or not the session uses them.

The manifest is a hint, always. When a runtime starts, refresh and reconcile. If a tool
the model called has vanished, that is a normal tool error — the same failure mode as a
server that changes tools mid-session, which already exists.

`warm(set)` / `validate(set)` on the host answers the error-timing question. Core agent
startup stays lazy; the desktop UI can proactively validate and surface failures at
session start the way `ExtensionLoadResult` does today. It must distinguish at least:

- failed to resolve configuration
- failed to start or connect
- started, but exposes none of the requested tools
- healthy

"No tools" is not automatically a failure — an extension may intentionally provide only
prompts or resources.

### Mutation: one authority

`manage_extensions` must **not** mutate the host. That would restore two authorities —
the caller's declared set and whatever the host happens to contain — which is the bug
this design exists to remove.

Instead the tool produces a typed desired-state mutation. The agent applies it to its own
selection and to persistence, then resolves a new lease. The host only ever satisfies
declarations.

**Freeze granularity is one inference request, not one user turn.** The set must be
stable across a single model call and the tool calls it produces, so the tool list the
model saw matches what dispatch resolves against. But once those calls finish, the *next*
inference must see the new tools — otherwise the model cannot use the extension it just
enabled until the user says something else, which defeats the feature.

### Sharing is a host policy

```rust
enum SharingPolicy {
    Scoped,   // scope id participates in RuntimeKey
    Shared,   // it does not
}
```

Stateless 2026-protocol HTTP servers can be `Shared`. Legacy HTTP, stdio and platform
extensions start — and platform extensions permanently stay — `Scoped`.

The two legacy blockers are exactly what MRTR removes. Roots are per-connection, above.
And server-to-client routing: when a legacy server calls `sampling/createMessage` or
`elicitation/create`, goose must know which session to route it to. It already puts the
session id in request `_meta` and reads it back off the callback
(`mcp_client.rs:272`), but falls back to the ambient `self.session_id`, guarded by
`assert!(... "McpClient received requests from different sessions")`. Sharing a legacy
connection means deleting that fallback, so any server that does not echo `_meta` loses
sampling and elicitation. The ambiguous case is already a hard error ("multiple tool
calls are active and the server did not echo the tool call request id").

Under MRTR neither applies: input requirements come back in the tool result and the
client retries, so there is no held-open stream to correlate. This is a per-runtime
negotiated policy, not a global switch.

## Lifecycle today

Worth recording because it is the weakest area. There is no `Drop for McpClient`, no
cancel, no shutdown handshake. Teardown relies entirely on `RunningService` being dropped
when the `Extension` leaves the HashMap, which kills the child. No graceful shutdown, no
reaping timeout, no idle eviction.

## Sequence and progress

1. **Done — cleanup.** The planning prompt method, frontend extensions, inline Python
   extensions and the `Sse` runtime variant were removed in
   [#11645](https://github.com/aaif-goose/goose/pull/11645).
2. **In review — extract transports.** One module per variant, each a
   `connect(&ConnectContext) -> Box<dyn McpClientTrait>`. `ConnectContext` replaces the
   parameter parade. Removes both `#[allow(too_many_arguments)]` and
   `#[allow(too_many_lines)]`. Implemented in
   [#12088](https://github.com/aaif-goose/goose/pull/12088).
3. **Next — resolve secrets once.** `add_extension` currently calls `config.resolve()` to
   compare against the stored snapshot, then each branch calls `merge_environments` and
   `substitute_env_vars` again to build the command. `resolve()` covers Stdio and
   StreamableHttp only; other branches inline it. Resolve once, build from the resolved
   config, and `Extension::resolved_config` stops needing to be kept in sync. This is
   where the sanitized connection fingerprint gets defined. Also in this step:
   `merge_environments` and `substitute_env_vars` leave `extension_manager/mod.rs` for
   wherever `ExtensionConfig::resolve` ends up, and `search_available_extensions` moves
   next to `config/extensions.rs` and the `manage_extensions` tool that calls it.
4. **Pending — `ExtensionSet` / `ExtensionScopeContext` / `ExtensionLease` /
   `ExtensionHost`.**
   Slots and the catalog, `session_id` off the signatures, host relocated above `Agent`,
   platform extension context rebuilt, and `manage_extensions` converted to emit a
   desired-state mutation. Scope id stays in `RuntimeKey` throughout.
5. **Pending — explicit lifecycle.** The three reference kinds, graceful shutdown with
   a timeout before SIGKILL, idle eviction of running-but-selected slots.
6. **Pending — manifest cache, lazy start, `warm`/`validate`.** The step users will feel.
7. **Pending — sharing policy.** `Shared` for stateless 2026 HTTP; everything else
   `Scoped`. The client already negotiates 2026-07-28 (`ClientLifecycleMode::Discover`),
   so this is not gated on protocol work.

## Constraints

`AGENTS.md` requires agent-loop changes to land in both `agent.rs` and `state_machine/`
until the migration completes. `ops_toolcalling.rs` and `ops_llm.rs` both call into the
manager, so future agent-loop changes, especially step 4, need parity in both paths.

Step 3 can land independently. Step 4 needs agreement on the issue first — specifically
on where `ExtensionHost` lives and what replaces `PlatformExtensionContext`.

## Open questions

- Does anything other than a session want to be an `ExtensionSetId` in practice?
  Subagents are the obvious candidate — `subagent_handler.rs` currently re-adds
  extensions to a fresh agent, respawning servers. Once the host lives above `Agent`, a
  subagent could lease a subset of the parent's slots. The id is opaque either way, so
  this only affects whether step 4 needs to prove the case.
- What is the default legacy fallback TTL, and is it per-transport? A stdio server that
  we start and stop constantly has different staleness economics from a remote HTTP one.
- Should `warm` be automatic for extensions the user has pinned or used recently, or
  strictly caller-driven?
