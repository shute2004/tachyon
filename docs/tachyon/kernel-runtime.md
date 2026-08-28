# Tachyon kernel runtime contract

Tachyon uses the word **Kernel** for the reusable coding-agent runtime that sits underneath complete coding-agent harness products.

This is not an operating-system-kernel analogy that implies the smallest possible feature set. Tachyon's kernel may be feature-rich. The architectural test is whether a capability is generally reusable across model providers and host/UI layers.

A complete product is expected to connect a host or frontend to this runtime rather than reimplement its agent behavior:

```text
CLI / TUI / Desktop / IDE / Web / other host
                    |
                    v
        provider-neutral host surface
                    |
                    v
              Tachyon Kernel
                    |
          model-execution boundary
                    |
        provider/protocol adapters
```

## What belongs in the kernel

The kernel owns mature coding-agent runtime capabilities that should behave consistently regardless of the frontend or model provider, including:

- thread, session, turn, and agent-loop lifecycle;
- context/history management, token budgeting, and compaction orchestration;
- tool specification, routing, execution, and lifecycle;
- steering, cancellation, retry, and recovery orchestration;
- permissions, approval, sandbox, shell/process/patch execution;
- persistence, resume, fork, and rollback semantics;
- MCP and extension lifecycle mechanisms;
- provider-neutral model execution and capability discovery;
- provider-neutral command, event, query, and snapshot surfaces needed by hosts.

The goal is that a host can supply presentation and product policy while reusing the complete runtime behavior instead of rebuilding a weaker harness around a thin SDK.

## What does not belong in the kernel

Presentation, product integration, provider wire details, and other non-reusable concerns stay outside the kernel or behind adapters. Examples include:

- CLI/TUI/Desktop/IDE/Web presentation and transport;
- ChatGPT/OpenAI account and product integration;
- concrete provider authentication and endpoints;
- OpenAI Responses, Anthropic Messages, or other wire protocols;
- provider/product-specific headers, routing tokens, compatibility behavior, analytics, updates, installation, and release concerns.

## Capability retention rule

Provider-specific implementation is not by itself a reason to delete code.

For each Codex-derived behavior, distinguish the semantic capability from its current realization:

1. **General harness capability** — retain it in the kernel behind a provider-neutral boundary.
2. **Provider-specific realization of a general capability** — retain the capability in the kernel and move the concrete realization behind the relevant adapter/private boundary.
3. **Host/UI or product-specific behavior** — move it outside the kernel.
4. **Provider/protocol-only behavior** — keep it in the provider/protocol layer rather than the kernel.
5. **Codex/OpenAI-only behavior with no reusable harness capability** — remove it once no supported behavior depends on it.

A provider-specific optimization should therefore survive when it realizes a generally useful harness capability. Conversely, behavior whose purpose is to privilege one provider/model in a way that reduces the quality, correctness, or freedom of other providers should not become a kernel contract unless a provider-neutral capability can be identified.

## Deletion gate

Do not delete mature Codex behavior merely because its current type, endpoint, header, or implementation is OpenAI-specific.

Before deleting a behavior during extraction, verify that at least one of the following is true:

- the capability has already moved behind a Tachyon-owned boundary and the old implementation is now redundant;
- the behavior belongs wholly to a provider/product layer that is being removed from the kernel dependency graph;
- the behavior has no reusable coding-agent capability and no supported Tachyon behavior depends on it.

If removing an OpenAI-specific mechanism would also remove a general capability such as runtime preparation, continuation, compaction, recovery, tool execution, permissions, sandboxing, or persistence, extract the capability first and delete only the obsolete realization afterward.

## Host boundary

UI independence does not mean that frontends should reach into kernel internals.

Tachyon should expose provider-neutral host-facing surfaces for the operations and state that real harness frontends require. Their exact stable API should be extracted from current runtime behavior rather than designed speculatively, but the ownership direction is fixed:

```text
host commands / queries
          |
          v
      Tachyon Kernel
          |
          v
host events / snapshots
```

Commands may eventually cover operations such as thread/turn lifecycle, steering, cancellation, approval decisions, tool-result submission, and rollback. Events and snapshots may expose turn progress, model output, tool lifecycle, approval requests, execution state, and persisted thread state. These examples describe responsibility, not a frozen API schema.

Concrete IPC, HTTP, app-server, CLI, TUI, or desktop transport belongs outside this kernel contract.

## Scope discipline

Small migration slices do **not** imply a minimal kernel.

Each PR should implement only the smallest change needed to move or validate one responsibility, while the final kernel preserves the full set of mature, generally reusable harness capabilities. Avoid both over-engineering a slice and over-pruning the runtime.
