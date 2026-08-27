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

Treat Endpoint as a potentially first-class responsibility, but do not equate first-class responsibility with a turn-construction-scoped resolved value.

The existence of an Endpoint abstraction and the lifetime of Endpoint resolution are separate design questions.

Current Codex code demonstrates that endpoint resolution has different timing and dependencies:

- `ModelProviderInfo::to_api_provider(...)` chooses a default base URL partly from the current auth mode.
- Amazon Bedrock can compute a runtime base URL dynamically from provider/auth/AWS state.
- request-attempt setup may be rebuilt during recovery, so an effective endpoint must remain re-resolvable when provider state changes.
- the current `codex_api::Provider` is a mixed request bundle containing base URL, query parameters, default headers, retry configuration, and stream idle timeout.

Therefore do not rename `codex_api::Provider` to a generic `Endpoint`, copy that whole shape into Tachyon, or add a resolved endpoint URL directly to `ModelRoute` merely to make the architecture look complete.

Long-term direction:

- Endpoint/deployment is distinct from Provider, Protocol, Auth, and Transport.
- effective Endpoint resolution may remain provider-owned and late-bound until request/connection execution.
- the generic kernel should preserve the ability for different request attempts to resolve different effective endpoints when provider state requires it.
- a request-attempt coordination object may eventually contain a resolved endpoint without making that object itself the semantic definition of Endpoint.

### Endpoint is not a protocol operation

Keep the provider deployment target separate from the protocol operation performed against it.

For example:

```text
ResolvedEndpoint
    https://api.openai.com/v1

Protocol operations
    /responses
    /responses/compact
```

A single `ModelTurnRuntime` may execute sampling, follow-up, retry, and compaction operations. Do not bake one operation path such as `/responses` into the endpoint identity.

### Query parameters

Do not classify every query parameter as Endpoint state.

Distinguish conceptually between:

- deployment-wide query defaults, such as an Azure API version;
- protocol-operation-specific query parameters;
- request-attempt-specific query decoration.

Preserve current behavior during migration before introducing a public taxonomy.

### Headers

Headers are not Endpoint identity. Preserve the distinction between:

- provider/deployment headers;
- protocol/request headers;
- auth/signing headers.

Maintain the established composition order when it is behaviorally significant, especially where request authentication signs the final URL, body, or headers.

### Retry and timeout

Retry configuration and stream idle timeout are execution policy, not Endpoint identity.

Keep low-level request/transport retry distinct from higher-level provider or harness recovery. A recovery path may need to re-resolve request-attempt setup instead of simply retrying the same physical target.

## Auth extraction

Endpoint and Auth should be separate responsibilities, but their resolution is allowed to depend on each other in phases.

Do not interpret separation as a requirement that endpoint selection and credential resolution be independently computable.

A safe conceptual ordering is:

```text
provider-private credential/runtime resolution
    -> endpoint selection/resolution
    -> protocol operation and final target construction
    -> request decoration
    -> request authentication/signing
    -> transport
```

Endpoint selection may depend on auth context, while final request authentication may depend on the resolved endpoint. Preserve this phase separation rather than forcing premature field independence.

## Provider-private runtime state

Continuation IDs, cached WebSocket state, refreshed credentials, entitlement data, provider discovery caches, signing state, sticky-routing tokens, and similar data stay opaque behind the adapter unless generic orchestration has a demonstrated need for a semantic result.

Expose capabilities and ownership semantics, not concrete provider field names or an untyped generic state bag.

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

For Endpoint extraction specifically, prefer internal decomposition of deployment location, request policy, request decoration, auth, and protocol operation before publishing a stable Tachyon-owned `ModelEndpoint` or `EndpointResolver`.

Do not design the final abstraction farther ahead than the code can currently justify.
