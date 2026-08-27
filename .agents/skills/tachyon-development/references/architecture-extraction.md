# Architecture extraction guidance

Use this reference for changes that move responsibilities out of Codex/OpenAI-specific implementation into Tachyon-owned model-runtime abstractions.

## Primary test

Apply the design test in both directions:

1. Do not promote a provider-specific mechanism into Tachyon's generic vocabulary merely because the current implementation uses it.
2. Do not delete a general harness capability merely because its current realization is provider-specific.

Examples:

- Responses WebSocket prewarm is specific; runtime preparation and reusable backend resources are general.
- `/responses/compact` is specific; compaction orchestration and optional remote compaction are general.
- `x-codex-turn-state` is specific; opaque fresh turn-affinity state is general.
- WebSocket-to-HTTP fallback is specific; retry/recovery orchestration is general.

## Runtime lifetime

`ModelRuntime` is session-scoped. `ModelTurnRuntime` is a fresh execution handle per harness turn.

The same turn runtime should be reused for sampling, tool follow-up, retry/recovery, and inline/remote compaction within that turn when those operations share turn-affinity state.

Fresh turn-private state must not leak between turns. Reusable backend state may be checked out by a turn and later returned to the session-scoped adapter. Preserve the existing `ModelClientSession::Drop` behavior until a replacement owns the same semantics.

## Provider / Protocol / Route

Provider identity, protocol identity, and transport are separate dimensions. `ModelRoute` currently represents a fully provider-bound logical execution route with:

- `ModelProviderId`
- `ModelProtocol`
- `ModelTransport`

Do not reintroduce a provider-less partial route just to support pre-turn capability checks.

## Endpoint extraction

Do not assume Endpoint is simply a fourth field on `ModelRoute`.

Current Codex code demonstrates that endpoint resolution has different timing and dependencies:

- `ModelProviderInfo::to_api_provider(...)` chooses a default base URL partly from the current auth mode.
- Amazon Bedrock can compute a runtime base URL dynamically from provider/auth/AWS state.
- the current `codex_api::Provider` is a mixed request bundle containing base URL, query parameters, default headers, retry configuration, and stream idle timeout.

Therefore do not rename `codex_api::Provider` to a generic `Endpoint` or copy that whole shape into Tachyon.

Before defining a stable `ModelEndpoint`, separate at least these concerns conceptually:

- endpoint/deployment location;
- request decoration such as query parameters and default headers;
- authentication/credentials;
- retry and timeout policy;
- protocol path selection;
- transport mechanics.

A request-attempt-scoped resolved endpoint may be more accurate than a turn-construction-scoped route field. Preserve this possibility until actual use sites establish the correct lifetime.

## Auth extraction

Auth may affect endpoint resolution in the current implementation. Do not force Endpoint and Auth into independent fields prematurely if doing so would duplicate or reorder existing resolution semantics.

The long-term goal is separation of responsibilities, not necessarily simultaneous or identical lifetime resolution.

## Provider-private runtime state

Continuation IDs, cached WebSocket state, sticky-routing tokens, and similar data stay opaque behind the adapter. Expose capabilities and ownership semantics, not concrete OpenAI field names.

## Migration shape

Prefer:

```text
existing behavior
    -> explicit narrow boundary
    -> existing implementation behind adapter
    -> production call-site migration
    -> removal of migration-only invalid states
    -> deeper decomposition
```

Do not design the final abstraction farther ahead than the code can currently justify.
