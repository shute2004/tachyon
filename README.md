# Tachyon

Tachyon is a model-agnostic, UI-independent runtime for building complete coding-agent harnesses.

It is being extracted from the OpenAI Codex codebase so that mature agent behavior can be preserved while model-provider, product, and user-interface coupling is moved behind explicit boundaries.

Tachyon calls this runtime the **Kernel**. The term does not mean an OS-style minimal core. Tachyon's Kernel is intended to retain the feature-rich execution capabilities that make a coding-agent harness useful; a CLI, TUI, desktop app, IDE integration, or other host should be able to attach to the Kernel without reimplementing the agent runtime.

```text
                    +-- CLI
                    +-- TUI
Tachyon Kernel -----+-- Desktop
                    +-- IDE
                    `-- Web / other hosts
```

## What the Kernel owns

Tachyon is intended to provide reusable coding-agent runtime capabilities such as:

- thread, session, turn, and agent-loop lifecycle
- context/history management and compaction
- model execution and capability discovery
- tool specification, routing, execution, and lifecycle
- steering, cancellation, retry, and recovery orchestration
- permissions, approvals, sandboxing, shell/process/patch execution
- persistence, resume, fork, and rollback semantics
- MCP and extension lifecycle mechanisms
- provider-neutral command/event surfaces for hosts and frontends

The criterion is **model/vendor/UI independence**, not minimum feature count.

Provider-specific implementations can still realize general harness capabilities. Tachyon preserves those capabilities behind adapters rather than deleting mature behavior merely because the current implementation originated in OpenAI Codex.

## What stays outside the Kernel

The Kernel does not directly own:

- CLI/TUI/Desktop/IDE/Web presentation
- ChatGPT/OpenAI account and product integration
- concrete provider authentication and endpoints
- provider wire protocols such as OpenAI Responses or Anthropic Messages
- OpenAI/Codex-specific headers, routing tokens, and compatibility behavior
- product analytics, update, installation, and release concerns

## Current status

Tachyon is under active extraction. The repository is public and buildable, but the standalone external Kernel API and workspace layout are not yet considered stable.

The current extraction has established a Tachyon-owned model-runtime boundary with separate session and turn lifetimes, and is progressively moving model request/event semantics through canonical provider-neutral IR while keeping unsupported provider-specific behavior on explicit adapter compatibility paths.

The broader extraction order is:

1. model-runtime seam and lifecycle
2. canonical model request/event IR
3. provider / protocol / route / auth / endpoint / transport separation
4. provider-neutral tool-call boundaries while retaining the mature tool runtime
5. agent loop behind Kernel-owned model/tool interfaces
6. context/history/compaction and persistence boundaries
7. execution, permissions, sandbox, and extension boundaries
8. removal of remaining Codex/OpenAI product dependencies from Kernel crates
9. standalone Tachyon workspace and external-use surfaces

Behavior preservation comes before internal redesign.

## Building the current workspace

Tachyon is still hosted inside the Codex-derived Rust workspace. From the repository root:

```shell
cd codex-rs
cargo check -p codex-core
```

`codex-rs/` and `codex-*` package names are transitional extraction names, not the intended final Tachyon workspace naming.

## Architecture documents

- [`TACHYON.md`](TACHYON.md) — project direction and current extraction state
- [`docs/tachyon/architecture.md`](docs/tachyon/architecture.md) — ownership rules, dependency direction, and extraction order
- [`docs/tachyon/model-ir.md`](docs/tachyon/model-ir.md) — canonical model request/event IR and migration boundary

## Development approach

Tachyon is not a clean-room rewrite. Development normally follows this sequence:

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
move harness call sites through the boundary
        |
        v
decompose provider/product details
```

Focused migration changes should preserve existing behavior and explicitly distinguish reusable harness capability from provider/product implementation detail.

## Origin and license

Tachyon is derived from [OpenAI Codex](https://github.com/openai/codex) and preserves its Apache-2.0 licensing basis.

This repository is licensed under the [Apache-2.0 License](LICENSE).
