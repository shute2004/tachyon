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

- role-bearing messages;
- text, image, and audio content;
- URI- or byte-backed media without assuming a provider upload mechanism;
- JSON-schema function tools and free-form tools;
- structured or textual tool-call input;
- text/JSON/media tool results;
- text and JSON-schema output contracts;
- streamed output-item lifecycle;
- text, tool-input, and reasoning deltas;
- request completion and provider-reported token usage.

The Rust definitions live in `codex-rs/core/src/model_runtime/ir.rs` during extraction.

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

## Migration order

The canonical IR is wired incrementally so existing Codex behavior remains a regression oracle.

### C1 — define and review the nucleus

- introduce kernel-owned request/event/data types;
- document provider-private exclusions;
- add focused unit coverage for the type semantics;
- do not change the production sampling path yet.

### C2 — request conversion boundary

- build `ModelRequest` in the harness for the supported common path;
- convert it to Responses request types only inside the Codex adapter;
- keep unsupported provider-specific history/state on an explicit transitional path rather than
  smuggling it into the canonical IR.

### C3 — event conversion boundary

- map generic stream lifecycle into `ModelEvent` in the adapter;
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
