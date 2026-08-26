# Tachyon architecture

Tachyon is a model-agnostic, UI-independent kernel for building coding-agent harnesses.

The project starts from the Codex codebase so that mature harness behavior can be extracted and generalized instead of reimplemented from scratch. Codex is the source substrate, not the target architecture or a permanent upstream dependency.

## Extraction strategy

Tachyon is developed by creating kernel boundaries inside the existing Rust workspace, moving reusable behavior behind those boundaries, and gradually turning Codex-specific code into consumers/adapters of the kernel.

The preferred migration pattern is:

```text
existing Codex behavior
        |
        v
new Tachyon boundary
        |
        v
existing implementation behind an adapter
        |
        v
incremental internal decomposition
```

Behavior preservation comes before internal redesign. Existing Codex tests and clients should remain useful as regression oracles while extraction is in progress.

## Kernel responsibilities

The kernel is expected to own generally reusable harness semantics, including:

- thread, session, turn, and run lifecycle
- agent loop and follow-up scheduling
- steering, cancellation, retry policy, and recovery
- context/history management and token budgeting
- compaction policy and lifecycle
- tool specification, routing, execution, and lifecycle
- permissions and approval orchestration
- shell/process/patch execution abstractions
- sandbox and network policy integration
- persistence, resume, fork, and rollback semantics
- MCP and extension lifecycle mechanisms
- model-runtime abstraction and model capability discovery
- provider-neutral command/event surfaces needed by frontends or hosts

The kernel may be feature-rich. The criterion is model/vendor/UI independence, not minimal feature count.

## Outside the kernel

The following belong outside the kernel or behind adapters/modules:

- CLI, TUI, Desktop, IDE, Web, and App Server presentation/transport layers
- ChatGPT/OpenAI account and product integration
- OpenAI-specific request headers and routing tokens
- concrete provider endpoints and authentication schemes
- provider wire protocols such as OpenAI Responses or Anthropic Messages
- product analytics, feedback, release/update, and installation concerns
- concrete model-catalog values that are provider/product-specific

## OpenAI-specific optimizations

"OpenAI-specific" is not by itself a reason to delete an implementation.

There are two materially different cases:

1. **Pure provider/product detail** — keep it outside the kernel in the relevant adapter or product layer.
2. **Provider-specific realization of a general harness capability** — preserve the capability in the kernel and move the concrete realization behind the appropriate boundary.

Examples:

- `x-codex-turn-state` is OpenAI/Codex-specific, but turn-affinity backend execution state is a general capability.
- `previous_response_id` is Responses-specific, but opaque provider continuation state is general.
- `/responses/compact` is OpenAI-specific, but compaction orchestration and optional remote compaction capability are general.
- Responses WebSocket prewarming is OpenAI-specific, but runtime preparation and reusable backend resources are general model-runtime concerns.
- Responses WebSocket-to-HTTP fallback is provider/transport-specific, but recovery policy and an execution backend's ability to recover or change execution route are general capabilities.

## Model-runtime direction

Tachyon must not promote Codex's current `ModelClient` shape into the kernel's universal model abstraction.

The current Codex implementation may initially be wrapped unchanged behind a new seam so that behavior is preserved:

```text
Agent Loop
   |
   v
Tachyon Model Runtime seam
   |
   v
Codex/OpenAI adapter
   |
   v
existing ModelClient
   |
   v
OpenAI Responses
```

After that seam is established, the existing `ModelClient` can be decomposed incrementally.

### Runtime lifetimes

The model-runtime boundary must preserve the distinct ownership roles already present in Codex without assuming that every object temporarily held by a turn runtime has a turn-only lifetime.

```text
Harness Session
    |
    v
ModelRuntime                         session-scoped
    |
    |-- session recovery/shared resources
    |-- reusable opaque backend cache
    |
    `-- begin_turn()
            |
            v
       ModelTurnRuntime              fresh handle per harness turn
            |
            |-- fresh turn-affinity state
            `-- exclusive checkout of reusable opaque backend state
                    |
                    `-- Drop returns reusable portion to ModelRuntime
```

For the initial OpenAI/Codex adapter, this maps approximately to:

```text
ModelRuntime
    `-- existing ModelClient
         +-- session-scoped provider/auth/recovery state
         `-- cached reusable WebsocketSession
              +-- WebSocket connection
              +-- last request
              +-- last response/continuation cache
              `-- incremental-request bookkeeping

ModelTurnRuntime
    `-- existing ModelClientSession
         +-- fresh x-codex-turn-state
         +-- checked-out WebsocketSession
         `-- turn execution ownership
              |
              `-- Drop returns reusable WebsocketSession to ModelClient
```

`x-codex-turn-state` is genuinely fresh per turn and must never leak into the next harness turn. By contrast, `previous_response_id`, the previous request/response cache, incremental request bookkeeping, and the WebSocket connection are implementation-private state whose useful lifetime may cross a harness-turn boundary. They can be temporarily owned by `ModelTurnRuntime` without becoming canonical turn-private Tachyon state.

The kernel must not introduce generic fields such as `turn_state: String`, `previous_response_id`, or a generic WebSocket cache. The generic capability is that a backend may own opaque session-scoped state, fresh turn-affinity state, and reusable resources that can be checked out by a turn handle and returned when that handle is dropped.

A fresh `ModelTurnRuntime` is created for each harness turn. The same handle is intended to be reused for sampling, tool follow-ups, retries, and inline/remote compaction inside that turn. Fresh turn-affinity state must not be reused across different harness turns, while reusable backend state may survive through the session runtime according to adapter-private rules.

For the initial adapter, the existing `ModelClientSession::Drop` behavior must be preserved. It returns the reusable `WebsocketSession` as a whole, not merely its connection, to the session-scoped `ModelClient` cache.

### Runtime preparation

Runtime preparation is a general optional capability. Responses WebSocket prewarming is one OpenAI-specific realization of that capability.

A backend may optionally prepare resources or opaque execution state before the first model request. The operation may be implemented differently by another backend or may be a no-op.

The current OpenAI/Codex path sends a Responses WebSocket `generate=false` warmup and transfers the prepared execution handle to the first regular turn. That concrete protocol and its `previous_response_id`/incremental reuse behavior remain adapter-private.

Generic APIs must not be named after Responses/WebSocket implementation details.

### Remote compaction

Compaction orchestration remains a harness concern. A model runtime may optionally provide a remote compaction capability.

The OpenAI adapter may continue to implement that capability with `/responses/compact` and its private sticky-routing state, but the model-runtime contract must not expose `x-codex-turn-state` merely so compaction can call the provider.

Conceptually:

```text
Harness Compaction Manager
        |
        +-- local compaction
        |
        `-- optional runtime remote-compaction capability
                |
                `-- OpenAI adapter
                     `-- /responses/compact + private turn state
```

### Retry and recovery

Retry scheduling, budgets, and user-visible recovery lifecycle are reusable harness concerns. The concrete mechanism used to recover a model request may remain provider/route/transport-specific.

The initial migration preserves existing retry and WebSocket-to-HTTP fallback behavior rather than making "switch fallback transport" a stable generic runtime contract. A later extraction step can introduce a provider-neutral recovery capability while leaving concrete route/transport changes in adapters.

## Long-term model execution shape

The intended long-term shape is approximately:

```text
Harness Session
   |
   v
Model Runtime
   |
   +-- canonical request/event/tool/usage data
   |
   `-- Model
        `-- Route
             +-- Provider
             +-- Protocol
             +-- Endpoint
             +-- Auth
             +-- Transport
             `-- provider-private runtime state
```

Provider and protocol are separate concepts. A provider may expose multiple protocols, and compatible deployments may reuse a protocol without sharing provider identity.

The model runtime does not own durable harness session history. Harness session state remains above the model-execution boundary.

Current `ModelProviderInfo`-style aggregates may remain inside migration adapters, but they must not be promoted as Tachyon's canonical Provider/Protocol/Endpoint/Auth/Transport model.

## Canonical model IR

Today, OpenAI Responses types reach deeply into Codex runtime, context, tools, and extension surfaces. Tachyon will gradually replace that coupling with provider-neutral request/event/item types.

This should be incremental rather than a clean-room rewrite. The first canonical types may intentionally resemble current Responses semantics. Support for additional providers will then expose which concepts are genuinely general and which belong in adapters.

The initial model-runtime seam is allowed to carry existing `Prompt`, `ResponseItem`, `ResponseEvent`, and related Responses-shaped types temporarily when that preserves behavior and keeps the migration small. Such types are migration-only compatibility surfaces and are not the stable Tachyon model IR.

Wire types must not become the stable kernel IR.

## Initial model-runtime migration phase

The initial migration is deliberately split so lifetime/API review is separate from behavior-preserving call-site rewiring.

### Step A — seam definition

This is the scope of the first Model Runtime PR.

In scope:

- introduce a Tachyon-owned model-runtime boundary
- represent the session-scoped runtime separately from a fresh per-turn runtime handle
- keep the existing `ModelClient` and `ModelClientSession` as the implementation behind the initial Codex/OpenAI adapter
- expose only the narrow operations needed to validate the model-execution shape
- document which state is fresh per turn versus opaque reusable backend state
- keep provider-private continuation, sticky-routing, WebSocket, auth-header, and request state below the runtime boundary
- mark current Responses-shaped request/event types as migration-only

Not in scope for Step A:

- rewiring `session/turn.rs`
- startup-prewarm ownership migration
- retry call-site migration
- remote-compaction call-site migration
- defining a generic recovery API

### Step B — call-site migration

The following PR should wire existing Codex behavior through the reviewed seam without changing behavior:

- make the agent sampling path depend on `ModelRuntime` / `ModelTurnRuntime` rather than concrete `ModelClientSession`
- preserve one turn-runtime handle across sampling, tool follow-up, retry, and inline compaction within a turn
- preserve startup preparation and ownership transfer into the first regular turn
- preserve cross-turn reusable backend cache semantics
- migrate retry/recovery without exposing a concrete WebSocket-to-HTTP mechanism as the final generic contract
- migrate remote compaction without exposing `x-codex-turn-state` through the Tachyon API
- preserve standalone compaction's independently owned runtime lifetime where required

Explicitly out of scope for the initial migration phase:

- stable canonical `ModelRequest` / `ModelEvent` design
- removal of every `ResponseItem` / `ResponseEvent` dependency
- final Provider abstraction
- final Protocol abstraction
- final Endpoint/Auth/Transport object model
- full retry architecture redesign
- model-catalog neutralization
- complete removal of existing provider metadata/capability references from `TurnContext`
- cleanup of all OpenAI product integration

The initial migration must not add new provider-private execution state above the runtime boundary. Existing provider metadata references that are not execution state may be deferred to later extraction steps.

## Dependency direction

Target dependency direction:

```text
frontends / products / hosts
          |
          v
      Tachyon Kernel
          |
          v
 model execution interfaces
          |
   +------+-------+
   |              |
providers      protocols
   |              |
   +------ routes-+
          |
      transports
```

Kernel crates must not depend on UI/TUI/Desktop concerns or concrete OpenAI product/account implementation.

During migration, temporary dependencies on existing `codex-*` crates are allowed when they let behavior be moved rather than rewritten. Those dependencies should be removed progressively once the kernel boundary owns the relevant abstraction.

## Extraction order

The current planned order is:

1. Establish a reproducible baseline and regression expectations.
2. Define and review the initial model-runtime seam without changing call sites.
3. Migrate sampling, preparation, retry, and compaction call sites through the model-runtime seam while preserving behavior.
4. Introduce provider-neutral model request/event representations.
5. Separate provider, protocol, route, auth, endpoint, transport, and runtime state.
6. Neutralize tool-call encoding while retaining Codex's mature tool runtime.
7. Move the agent loop behind kernel-owned model/tool interfaces.
8. Neutralize context/history/compaction and persistence surfaces.
9. Consolidate execution, permissions, sandboxing, and extension boundaries.
10. Remove remaining Codex/OpenAI product dependencies from kernel crates.
11. Extract the standalone Tachyon workspace structure.

## Naming migration

Codex-derived names should be neutralized incrementally as semantic ownership moves into Tachyon. Naming follows architectural ownership rather than cosmetic rebranding.

- Rename a `Codex*`, `codex_*`, or `codex-*` identifier when the concept it names has become a genuinely model/vendor/UI-independent Tachyon concern and the touched boundary makes that ownership clear.
- Keep explicit Codex/OpenAI/Responses naming when an identifier still represents a transitional adapter, provider/product integration, wire protocol, header, endpoint, compatibility behavior, or other intentionally specific implementation detail.
- Prefer opportunistic, local renames in the same PR that moves the corresponding responsibility across a boundary. Avoid repository-wide mechanical renames that mix semantic extraction with unrelated churn.
- Do not rename merely to hide ancestry. A specific name is useful when it tells future maintainers that the implementation still belongs below an adapter boundary.
- Once a generic API replaces a Codex-specific API, new kernel code should use the neutral name and callers should migrate toward it rather than introducing new Codex-branded aliases above the boundary.

Examples during the model-runtime migration:

- a session service accessor that exposes the generic boundary should be named `model_runtime`, even while its transitional backing field is still `model_client`;
- `ModelClient` and `ModelClientSession` retain their current names while they are the concrete Codex/OpenAI implementation behind `ModelRuntime` / `ModelTurnRuntime`;
- Responses-specific metadata, headers, continuation identifiers, and transport operations retain explicit provider/protocol naming until their generic capability is extracted.

## Development workflow

- `main` should stay build/test-capable.
- Non-trivial changes use a focused GitHub issue, a short-lived branch, and a pull request.
- Prefer one issue per branch/PR when practical.
- Important architectural issues and PRs receive an independent LLM review before merge.
- Mechanical moves/renames do not require redundant architecture review unless they alter boundaries or behavior.
- A migration PR should state what behavior is intentionally unchanged, what dependency direction changed, and what remains deliberately deferred.

## Baseline

The first Tachyon extraction work begins from the fork's `main` at commit `3ba7b6941d3caf6eec5b3c4e564988ee57d3f083`.

This commit is a reference point, not a promise to keep tracking Codex upstream. Tachyon may selectively incorporate useful upstream changes while extraction is underway, but upstream compatibility is not a project goal.
