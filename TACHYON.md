# Tachyon

Tachyon is a model-agnostic, UI-independent kernel for building coding-agent harnesses.

It is written in Rust and is being extracted from the OpenAI Codex codebase so that mature coding-agent runtime behavior can be preserved while product, UI, and provider-specific coupling is progressively removed.

Codex is the extraction substrate, not Tachyon's target architecture and not a permanent upstream dependency.

## What Tachyon is

Tachyon is intended to provide the reusable runtime underneath complete coding-agent products.

Conceptually:

```text
                    +-- CLI
                    +-- TUI
Tachyon Kernel -----+-- Desktop
                    +-- IDE
                    `-- Web / other hosts
```

The kernel may be feature-rich. The goal is not a minimal SDK. The criterion is that kernel responsibilities are reusable across model providers and user-interface layers.

Expected kernel responsibilities include:

- thread, session, turn, and agent-loop lifecycle
- context/history management and compaction
- tool specification, routing, execution, and lifecycle
- steering, cancellation, retry policy, and recovery orchestration
- permissions, approval, sandbox, shell, process, and patch execution
- persistence, resume, fork, and rollback semantics
- MCP and extension lifecycle mechanisms
- provider-neutral model execution interfaces and capability discovery

## What stays outside the kernel

Tachyon should not directly own product or provider details such as:

- CLI/TUI/Desktop/IDE/Web presentation
- ChatGPT/OpenAI account integration
- provider-specific authentication and endpoints
- OpenAI Responses, Anthropic Messages, or other provider wire protocols
- OpenAI/Codex-specific request headers, routing tokens, and compatibility behavior
- product analytics, feedback, update, and release concerns

Provider-specific implementations may still realize general harness capabilities. Those implementations should be preserved behind adapters rather than discarded merely because the current implementation is OpenAI-specific.

## Extraction strategy

Tachyon is not being rewritten from scratch.

The migration pattern is:

```text
mature Codex behavior
        |
        v
introduce a Tachyon-owned boundary
        |
        v
keep the existing implementation behind an adapter
        |
        v
move harness call sites through the new boundary
        |
        v
decompose provider/product details behind adapters
        |
        v
remove remaining Codex product dependencies
```

Behavior preservation comes before internal redesign.

## Current stage

The first major boundary is the model runtime.

Step A introduced and reviewed two execution lifetimes:

```text
ModelRuntime                    session-scoped
    |
    `-- begin_turn()
            |
            v
       ModelTurnRuntime         fresh handle per harness turn
```

Step B moved the regular agent turn, startup preparation, sampling retry/recovery, and inline/standalone compaction through that runtime boundary while preserving the existing Codex `ModelClient` / `ModelClientSession` implementation behind the adapter.

The current runtime ownership is therefore:

```text
ModelRuntime
    |
    `-- ModelTurnRuntime
          +-- startup preparation
          +-- pre-turn compaction
          +-- sampling
          +-- tool follow-up
          +-- retry / recovery
          `-- inline compaction
```

The project is now entering Step C: introducing canonical provider-neutral model request/event data without promoting the existing OpenAI Responses shapes into Tachyon's stable kernel vocabulary.

The model-runtime source layout now includes the first canonical IR nucleus:

```text
codex-rs/core/src/model_runtime/
├── mod.rs              # Tachyon-facing runtime boundary
├── ir.rs               # canonical provider-neutral request/event vocabulary
├── ir_tests.rs         # focused IR semantics tests
├── codex_adapter.rs    # transitional Codex/OpenAI implementation
├── retry.rs            # model-stream retry policy
└── retry_tests.rs      # retry policy tests
```

The production sampling path still uses migration-only `Prompt` / `ResponseItem` / `ResponseEvent` shapes while adapter conversions are introduced incrementally. Provider / Protocol / Endpoint / Auth / Transport decomposition follows after the execution IR is established.

See [`docs/tachyon/model-ir.md`](docs/tachyon/model-ir.md) for the canonical model IR boundary and migration order.

## Naming during extraction

Codex-derived names are neutralized incrementally when semantic ownership moves into Tachyon.

A broad repository-wide rename would hide which concepts are already generic and which still represent Codex/OpenAI-specific behavior. Therefore:

- generic harness concepts should acquire Tachyon/provider-neutral names when their boundary is extracted;
- files, modules, functions, variables, types, directories, workspace names, and crate names follow the same rule;
- `codex-rs/` and `codex-*` crate/package names are migration artifacts and are expected to be neutralized once the corresponding crate boundaries and dependency directions are stable;
- names that still represent Codex/OpenAI adapters, Responses protocol details, headers, endpoints, compatibility behavior, or product integration remain explicit until that responsibility moves behind an adapter.

See [`docs/tachyon/architecture.md`](docs/tachyon/architecture.md) for the detailed architecture, ownership rules, migration phases, and dependency direction.
