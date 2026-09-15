# Extension manager redesign

Status: proposal, revised twice after review. Nothing here is implemented yet.

## Why

`crates/goose/src/agents/extension_manager.rs` is 2,720 lines before the test module.
It is a `Mutex<HashMap<String, Extension>>` plus everything that has ever needed to
touch it. Seven distinct jobs live in one file:

1. Transport construction — process spawn, PATH resolution, stderr capture, docker
   exec, unix sockets, and the OAuth refresh / 401 / browser-fallback dance. This is
   everything above `impl ExtensionManager` at line 1370.
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

Three consequences we want to fix:

**`add_extension` is the wrong shape.** A 280-line `match` under
`#[allow(clippy::too_many_lines)]` that interleaves config resolution, secret merging,
env substitution, docker-vs-local, malware checking, temp-file writing, transport
setup and registry insert. Every branch calls something with 9-12 positional arguments
under `#[allow(clippy::too_many_arguments)]`. The same six values — `provider`,
`client_name`, `capabilities`, `working_dir`, `action_required`,
`Weak<ExtensionManager>` — travel together everywhere. That is a struct that does not
exist yet.

**"Which extensions are in play" is expressed four incompatible ways.** The
`extension_name` filter argument, the `exclude` argument (added for code mode), the
`unprefixed_tools` flag and the `hidden` flag. Each is a special case bolted onto a
global catalog.

**There are two entry points to "enable an extension" and they disagree.**
`Agent::add_extension` fetches the session, passes `working_dir`, the container and the
session id, then persists session state. `ExtensionManagerClient::manage_extensions_impl`
upgrades the `Weak<ExtensionManager>` out of `PlatformExtensionContext` and calls
`add_extension(config, None, None, None)`
(`crates/goose/src/agents/platform_extensions/ext_manager.rs:191`). So when the *model*
enables an extension it starts in the process cwd instead of the session working dir,
outside the docker container, without `AGENT_SESSION_ID`, and is never persisted.
Disable has the mirror problem: it skips `remove_frontend_extension` and persistence.
This is a live bug and it is a direct consequence of the layering.

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

goose today is an initialize-based client (`McpClient::connect` → `serve` →
`peer_info`). Supporting the 2026 protocol is separate work; this redesign should avoid
making it harder, which mostly means not baking the old handshake into the cache format.

## Two things to delete

### The planning prompt

`get_planning_prompt` (`extension_manager.rs:2060`) has exactly one caller
(`agent.rs:3960`). But `plan.md` itself is a documented, user-editable prompt template,
listed in `documentation/docs/guides/context-engineering/prompt-templates.md` as
CLI-only. So delete the method, keep the template, render it at the call site or in
`prompt_manager`. No feature loss.

### Frontend extensions

Dead in every current front end. ACP skips the variant when listing
(`crates/goose/src/acp/server/extensions.rs:248`) and hard-bails on it in recipes
(`crates/goose/src/acp/server/recipe/conversions.rs:457`). The CLI has no front end to
call back into. Nothing outside tests constructs one.

The runtime blast radius is wide and all of it is subtraction:

- `MessageContent::FrontendToolRequest`, matched in seven provider formats (anthropic,
  openai, openai_responses, databricks, snowflake, bedrock, openrouter).
- The `is_frontend_tool` branch in `crates/goose/src/agents/tool_execution.rs:250`.
- `frontend_instructions` threading through `prompt_manager` and both agent loops.
- `Agent`'s `frontend_extensions` / `frontend_tools` / `frontend_instructions` fields
  and the union logic in `list_tools`, `list_extensions`, `get_extension_configs`,
  `total_extension_and_tool_counts`, `add_extension_inner` and `remove_extension` —
  roughly 120 lines.

**The risk is deserialization of historical data, not runtime.** Both
`ExtensionConfig` and `MessageContentBlock` are `#[serde(tag = "type")]`, so removing a
variant means old data mentioning it stops parsing. Four call sites, and they differ:

- **Config file — safe.** `parse_extensions_map`
  (`crates/goose/src/config/extensions.rs:54`) parses each entry individually and logs
  and skips on error. A stale `type: frontend` entry just disappears.
- **Session extension state — silent data loss.** `EnabledExtensionsState` holds a
  `Vec<ExtensionConfig>` deserialized as one unit, and
  `ExtensionState::from_extension_data` swallows the error with `.ok()`
  (`crates/goose/src/session/extension_data.rs:69`). One unrecognised variant anywhere
  in the vec makes the whole state return `None`, and `extensions_or_default` silently
  falls back to global config. The session does not crash — it quietly loses its
  per-session extension set with no error message.
- **Persisted conversations — hard failure.** This is the worst one.
  `session_manager.rs:1887` deserializes each message's content with
  `serde_json::from_str(&content_json)?` inside the row loop, and the `?` propagates out
  of the whole function. A single historical message containing a
  `frontendToolRequest` block makes the **entire session fail to load**, not just lose
  one message. Frontend tools were used by the pre-ACP Electron UI, so such messages
  plausibly exist in real users' history.
- **Recipes — already loud.** `recipe_extension_adapter.rs` carries its own
  `#[serde(rename = "frontend")]` and ACP already bails, so failure here is loud and
  pre-existing.

Fix, in two parts:

- Keep `FrontendToolRequest` as a **deserialize-only tombstone** in
  `MessageContentBlock`, rendered as inert text. Remove every piece of runtime handling;
  keep the wire variant so history still loads. This is not optional.
- For session extension state, use per-element tolerant deserialization (or an
  `Unknown` tombstone variant) so unrecognised entries drop individually with a warning
  instead of taking the whole list down.

Removing `Sse` has a smaller version of the same problem: `get_warnings`
(`crates/goose/src/config/extensions.rs:275`) matches specifically on
`ExtensionConfig::Sse` to tell users to migrate to `streamable_http`. Delete the variant
and that targeted message degrades to a generic "skipping malformed entry" log. Keep a
config-level tombstone if we still want to nudge people.

## Target design

### Declaration and lease are two types

The first draft called both of these `ExtensionSet` and claimed refcounting "falls out
for free." Both were wrong. A plain value cannot provide refcounted release, and an
`Arc` held by the host does not by itself bound idle lifetime.

```rust
struct ExtensionSet {                  // persisted desired state
    id: ExtensionSetId,                // opaque scope
    working_dir: PathBuf,
    execution_target: ExecutionTarget, // local, or a container identity
    extensions: Vec<ExtensionSelection>,
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
    members: Vec<ExtensionHandle>,     // handles refer to slots, not clients
    catalog: ToolCatalog,
}
```

```rust
let lease = host.resolve(&extension_set, &scope_context).await?;
lease.tools().await?;
lease.call(name, params).await?;
```

`ExtensionSet` is inert data the agent owns and persists. `ExtensionScopeContext` carries
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
  replace it.
- Dropping or replacing the old lease releases its selection references.

That is what makes the freeze rule concrete.

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
    selected_by: HashSet<ExtensionSetId>,
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

Either way `update_working_dir` (`extension_manager.rs:1778`), which mutates that shared
lock and fires `roots/list_changed`, stops existing. Changing directory means presenting
a set with a different `working_dir`.

### The lease owns the catalog

Methods live on the resolved lease, not on a manager taking a set. `session_id` comes
off every signature — it is not a real dimension today, since there is one `Agent` per
ACP session (`crates/goose/src/acp/server.rs:1221`) and `McpClient` asserts it never
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

Cache a goose-owned type, not a protocol type, and cache per section rather than as one
blob — the protocol gives tools and discovery their own freshness.

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
`Agent::add_extensions_bulk` (`agent.rs:1508`) spawns every configured server before the
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

## Sequence

Steps 1-4 are refactors with no user-visible behaviour change and need no flag.

1. **Delete.** Planning prompt method, frontend extensions, `Sse` variant — with the
   `FrontendToolRequest` message tombstone and the session-state tolerance described
   above. Largest diff; the risk is entirely historical-data compatibility, and it is
   now identified.
2. **Extract transports.** One module per variant, each a
   `connect(&ConnectContext) -> Box<dyn McpClientTrait>`. `ConnectContext` replaces the
   parameter parade. Removes both `#[allow(too_many_arguments)]` and
   `#[allow(too_many_lines)]`.
3. **Resolve secrets once.** `add_extension` currently calls `config.resolve()` to
   compare against the stored snapshot, then each branch calls `merge_environments` and
   `substitute_env_vars` again to build the command. `resolve()` covers Stdio and
   StreamableHttp only; other branches inline it. Resolve once, build from the resolved
   config, and `Extension::resolved_config` stops needing to be kept in sync. This is
   where the sanitized connection fingerprint gets defined.
4. **`ExtensionSet` / `ExtensionScopeContext` / `ExtensionLease` / `ExtensionHost`.**
   Slots and the catalog, `session_id` off the signatures, host relocated above `Agent`,
   platform extension context rebuilt, and `manage_extensions` converted to emit a
   desired-state mutation. Scope id stays in `RuntimeKey` throughout.
5. **Explicit lifecycle.** The three reference kinds, graceful shutdown with a timeout
   before SIGKILL, idle eviction of running-but-selected slots.
6. **Manifest cache, lazy start, `warm`/`validate`.** The step users will feel.
7. **Sharing policy.** `Shared` for stateless 2026 HTTP; everything else `Scoped`. Gated
   on 2026 protocol support existing.

## Constraints

`AGENTS.md` requires agent-loop changes to land in both `agent.rs` and `state_machine/`
until the migration completes. `ops_toolcalling.rs` and `ops_llm.rs` both call into the
manager, so steps 1 and 4 need parity work in both paths.

Steps 1-3 can go up as one PR each. Step 4 needs agreement on the issue first —
specifically on where `ExtensionHost` lives and what replaces
`PlatformExtensionContext`.

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
