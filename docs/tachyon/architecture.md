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

- `x-codex-turn-state` is OpenAI/Codex-specific, but turn-scoped backend execution state is a general capability.
- `previous_response_id` is Responses-specific, but opaque provider continuation state is general.
- `/responses/compact` is OpenAI-specific, but compaction orchestration and optional remote compaction capability are general.
- Responses WebSocket prewarming is OpenAI-specific, but reusable connection/session state and transport fallback are general model-runtime concerns.

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

## Canonical model IR

Today, OpenAI Responses types reach deeply into Codex runtime, context, tools, and extension surfaces. Tachyon will gradually replace that coupling with provider-neutral request/event/item types.

This should be incremental rather than a clean-room rewrite. The first canonical types may intentionally resemble current Responses semantics. Support for additional providers will then expose which concepts are genuinely general and which belong in adapters.

Wire types must not become the stable kernel IR.

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
2. Introduce the initial model-runtime seam without changing behavior.
3. Introduce provider-neutral model request/event representations.
4. Separate provider, protocol, route, auth, endpoint, transport, and runtime state.
5. Neutralize tool-call encoding while retaining Codex's mature tool runtime.
6. Move the agent loop behind kernel-owned model/tool interfaces.
7. Neutralize context/history/compaction and persistence surfaces.
8. Consolidate execution, permissions, sandboxing, and extension boundaries.
9. Remove remaining Codex/OpenAI product dependencies from kernel crates.
10. Extract the standalone Tachyon workspace structure.

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