# Tachyon

Tachyon is a model-agnostic, UI-independent kernel for building coding-agent harnesses.

This repository currently uses the Codex codebase as an extraction substrate. The goal is not to maintain a permanently compatible Codex fork: reusable harness behavior is being separated behind model-neutral, UI-independent Rust interfaces until the kernel can stand alone.

See [`docs/tachyon/architecture.md`](docs/tachyon/architecture.md) for the architectural boundaries, extraction strategy, model-runtime direction, and development workflow.

## Current extraction stage

The initial stage is intentionally behavior-preserving:

```text
Codex runtime
    |
    v
introduce Tachyon-owned seams
    |
    v
keep existing Codex implementations behind adapters
    |
    v
incrementally remove product/provider coupling
```

The first major technical seam will separate the agent loop from Codex's concrete `ModelClient` without yet rewriting the existing OpenAI implementation.