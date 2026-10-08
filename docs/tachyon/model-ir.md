# Canonical model IR

Tachyon's model IR is the kernel-owned vocabulary between the harness and model-runtime adapters.
It is not a renamed copy of OpenAI Responses, Anthropic Messages, or any other provider wire
protocol.

The IR should describe semantics that a coding-agent harness needs to issue model work and consume
model output while allowing each adapter to choose the provider/protocol representation.

## Boundary

```text
Harness / agent loop
        |
        v
canonical ModelRequest / ModelEvent
        |
        v
Model Runtime adapter
        |
        +-- OpenAI Responses
        +-- Anthropic Messages
        +-- Gemini
        `-- other/local backends
```

Provider and protocol details belong below the canonical IR boundary.

## First canonical nucleus

The first IR slice intentionally covers only concepts whose harness meaning is already clear:

- role-bearing messages, including optional commentary/final lifecycle semantics;
- text, image, and audio content;
- URI- or byte-backed media without assuming a provider upload mechanism;
- JSON-schema function tools and free-form tools;
- free-form input grammar constraints without assuming a provider wire representation;
- immediate versus deferred tool availability and semantic tool-discovery purpose;
- structured or textual tool-call input;
- text/JSON/media tool results with a tri-state error marker (`Some(true)` known error,
  `Some(false)` known success, `None` unknown);
- tool-discovery results that expose semantic tool declarations without carrying provider-shaped
  serialized tool JSON;
- text and JSON-schema output contracts;
- streamed output-item lifecycle with a partial start header distinct from a completed item;
- text, tool-input, and reasoning deltas;
- request completion and provider-reported token usage.

The Rust definitions live in the `tachyon-model` crate at
`codex-rs/tachyon-model/src/lib.rs`. This crate depends only on `std` and `serde_json`,
not on Core, provider protocols, authentication, or UI crates. Core's
`codex-rs/core/src/model_runtime/ir.rs` re-exports those same types for existing callers;
there is no second IR definition or conversion between the two import paths.

The existing provider, protocol, transport, and route identities likewise belong to
`tachyon-model::route`; Core retains `model_runtime::route` as a compatibility re-export.
Endpoint resolution, authentication, and concrete transport execution remain adapter-owned.

The existing twelve IR semantics tests live with the owning crate. Core separately checks
cross-path type identity and its adapter conversions. This ownership move does not yet make
the complete model runtime or agent loop independent of Codex.

## Executable backend boundary

`tachyon-model::backend` defines a session-scoped `ModelBackend` factory, a fresh
provider-bound `ModelTurnBackend` for each harness turn, and an ordered canonical event
source. A selected model ID is passed separately from the provider identity. Backends own
their endpoint, authentication, transport, and reusable private resources; these do not
cross the canonical contract.

A host selects this runtime with `ModelRuntime::from_backend` and
`StartThreadOptions::with_model_runtime`. Session startup snapshots that selection, rather
than looking it up from mutable extension data on every turn. The normal sampling path
keeps one backend handle across tool follow-ups and checks its provider identity before
each request. Fatal or unsupported requests are not retried; retryable backend failures
use the existing bounded stream retry policy.

Canonical output reaches the existing history and tool handlers through a transitional
projection, without invented Codex event context or response IDs. Raw Responses telemetry
remains exclusive to the Codex path. The Codex adapter retains its lossless legacy fallback;
a canonical backend instead rejects prompts containing unsupported history or tool declarations.
Function tool declarations retain their optional JSON output schemas. Explicit namespace
groups preserve descriptions, empty groups, duplicate group boundaries and declaration order;
their children do not declare another namespace. The Codex adapter rejects nested groups and
discovery children instead of inventing conflict-resolution semantics. Legacy flat namespaced
declarations retain their existing grouping behavior, separately from explicit groups.
Hosted WebSearch remains unsupported by this canonical sampling slice. Client-discovery output
retains its existing flat vocabulary and rejects explicit groups or output-schema declarations;
this request-side extension does not silently expand the discovery protocol.

This slice does not implement canonical compaction, reasoning-content section starts, or
automatic backend inheritance for delegated child threads. Those operations remain
explicitly unsupported or on their existing Codex paths. Realtime and delegated WebSocket
capability checks still retain the legacy client. The complete agent loop is not yet a
standalone provider-neutral crate.

Tool runtimes use a separate result-side vocabulary for client discovery: `ToolResultContent::DiscoveredTools`
contains result-specific semantic function/free-form declarations with namespace, schema or grammar,
strictness, and immediate/deferred availability. The Codex adapter converts those declarations to
Responses `ToolSearchOutput` only at the provider boundary; an unrepresentable declaration keeps the
existing legacy output path.

## Preserve generic capability, not provider realization

Provider-specific realization must not be copied mechanically into the IR, but generic harness
capabilities underneath it must remain representable.

Current Codex behavior provides three useful examples:

- Responses custom/free-form tools carry a concrete format object. The canonical capability is that
  free-form input may be unconstrained text or constrained by a grammar, represented by
  `ModelFreeformInputFormat`; the Responses `type` field itself is not canonical.
- Codex uses Responses `defer_loading` and a `tool_search` wire tool to implement client-side
  deferred tool discovery. The canonical capability is represented by `ModelToolAvailability`,
  `ModelToolPurpose`, and semantic discovered-tool declarations in `ModelToolResultContent`, not by
  Responses-shaped `ToolSearch` / `ToolSearchOutput` variants or serialized provider tool JSON.
- Responses assistant messages may carry `Commentary` or `FinalAnswer`. Tachyon keeps the
  harness-significant distinction as `ModelMessagePhase::{Commentary, Final}` while allowing `None`
  for providers that do not expose it.

## Stream starts are partial

A streamed item can begin before all of its completed value is available. This matters especially
for structured tool calls: JSON arguments may still be an incomplete fragment when the item starts.

`OutputItemStarted` therefore carries `ModelOutputItemStart`, a header with correlation and input-kind
information, while `OutputItemCompleted` carries a complete `ModelOutputItem`. Adapters do not need
to invent placeholder JSON or delay the start event until tool input has finished streaming.

## Correlation IDs are not provider IDs

`ModelItemId` and `ModelToolCallId` are runtime correlation identifiers. They must not acquire wire
semantics such as Responses `response_id` or a provider-specific item-ID prefix.

An adapter may preserve a provider ID when it is useful and safe, or mint a runtime correlation ID
when a provider does not expose one. Provider continuation identity stays private to the adapter.

## Deliberately excluded

The canonical IR must not grow fields or variants merely because the current Codex backend needs
them. In particular, the following remain outside this IR unless a later multi-provider design
identifies a genuinely generic harness capability:

- `x-codex-turn-state` and other sticky-routing tokens;
- Responses `previous_response_id`;
- Responses WebSocket/session objects and incremental-request caches;
- `/responses/compact` request/response shapes;
- OpenAI `ModelsEtag` updates;
- OpenAI/Codex account verification and moderation presentation metadata;
- provider-specific rate-limit/account-plan payloads;
- concrete WebSocket-to-HTTP fallback details;
- provider-specific web-search and image-generation wire payloads;
- Codex internal chat-message passthrough metadata;
- encrypted provider continuation/reasoning blobs as generic model fields.

Some of these may correspond to generic capabilities. For example, provider-private continuation
state is useful, but the generic capability is that the adapter/runtime may retain opaque state —
not that `ModelRequest` has a `previous_response_id` field.

## Product/backend side events

The existing Codex `ResponseEvent` mixes model-execution events with backend/product notifications.
Examples include server-model mismatch data, model verification, moderation metadata, model-catalog
ETags, and rate-limit/account state.

Those notifications must not be copied wholesale into `ModelEvent`. During migration they may
continue through Codex-specific compatibility paths. Later they should either:

1. be handled entirely below the model-runtime boundary;
2. map to a separately justified generic harness capability; or
3. remain a product/host side channel outside the kernel model-event stream.

## Usage semantics

`ModelUsage` distinguishes unknown optional token classes from a reported value of zero. Input and
output tokens are the common baseline; cache and reasoning details are optional. A provider-reported
total may be kept separately rather than recomputed because providers can account for token classes
that the common fields do not represent.

Tool-result status follows the same lossless principle at the model-runtime boundary. A canonical
`ModelToolResult` keeps `is_error: Option<bool>`: `Some(true)` and `Some(false)` retain an explicit
provider or harness decision, while `None` means that the producer did not determine the status.
The current Responses adapter maps this marker back to `FunctionCallOutputPayload.success` without
turning an unknown result into a success or error.

## Migration order

The canonical IR is wired incrementally so existing Codex behavior remains a regression oracle.

### C1 — define and review the nucleus

- introduce kernel-owned request/event/data types;
- document provider-private exclusions;
- preserve existing generic harness semantics that are currently realized through Responses-specific
  tool/message fields;
- add focused unit coverage for the type semantics;
- do not change the production sampling path yet.

### C2 — request conversion boundary

- build `ModelRequest` in the harness for the supported common path;
- convert it to Responses request types only inside the Codex adapter;
- preserve grammar-constrained free-form tools and deferred tool discovery through the canonical
  semantics rather than Responses wire variants;
- keep unsupported provider-specific history/state on an explicit transitional path rather than
  smuggling it into the canonical IR.

### C3 — event conversion boundary

- map generic stream lifecycle into `ModelEvent` in the adapter;
- preserve assistant commentary/final phase where the backend reports it;
- use partial `ModelOutputItemStart` for stream starts and complete values for completion;
- keep Codex/OpenAI product notifications outside the canonical event stream;
- migrate the agent loop away from matching `ResponseEvent` directly.

### C4 — expand only from demonstrated harness requirements

- add remaining tool/content/model capabilities when the current harness or a second adapter needs
  them;
- use additional providers to test the abstraction instead of pre-designing every possible wire
  feature.

## Non-goals of the first IR slice

- supporting every current `ResponseItem` variant immediately;
- defining the final provider/model capability system;
- defining Provider / Protocol / Route / Auth / Endpoint / Transport in the same PR;
- replacing persistence/history schemas;
- changing retry, compaction, tool execution, or agent-loop behavior.
