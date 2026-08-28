## Contributing

Tachyon is under active extraction from the OpenAI Codex codebase. The repository is public, but its long-term contribution and governance model is not yet considered stable.

This document describes the expectations for changes made in the repository today. It does not promise that every proposed external change will be accepted while the kernel boundary is still moving.

### Before changing code

Read the project architecture first:

- [`TACHYON.md`](../TACHYON.md)
- [`docs/tachyon/architecture.md`](tachyon/architecture.md)
- [`docs/tachyon/model-ir.md`](tachyon/model-ir.md) when changing model-runtime request or event semantics

Tachyon is an extraction project rather than a clean-room rewrite. Changes should preserve mature harness behavior unless the change is intentionally modifying that behavior.

### Scope changes narrowly

Prefer focused changes that move one responsibility behind a Tachyon-owned boundary or fix one concrete behavior.

In particular:

- do not remove a mature Codex capability merely because its current implementation is OpenAI-specific;
- distinguish reusable harness semantics from provider/product wire details;
- keep provider-specific protocol, authentication, endpoint, transport, and compatibility behavior below the corresponding boundary;
- avoid introducing speculative generic abstractions before the current code provides evidence for their semantics;
- avoid unrelated cleanup in architecture-migration changes.

When an existing compatibility path is intentionally retained, make that explicit rather than forcing a provider-specific shape into a generic Tachyon type.

### Validate the affected workspace

The current Rust workspace remains under `codex-rs/` during extraction.

Start validation from the repository root and use the repository-pinned Rust toolchain:

```shell
cd codex-rs
cargo check -p codex-core
```

Add focused tests for the behavior or boundary being changed. Broader checks should be proportional to the affected dependency surface rather than added speculatively.

The existing CI configuration is inherited from the Codex-derived workspace and remains part of the migration surface. Some jobs cover the full workspace rather than only Tachyon-specific code.

### Architecture-sensitive changes

Changes to the following areas should be reviewed as architecture changes rather than only as local refactors:

- model-runtime interfaces or canonical IR
- provider / protocol / route / endpoint / auth / transport ownership
- tool-call semantic boundaries
- agent-loop ownership
- context/history/compaction or persistence boundaries
- host-facing command/event/query/snapshot contracts
- removal of Codex/OpenAI implementation that may contain reusable harness capability

For those changes, review should consider the exact final diff and current base, because a review performed before a material rebase or follow-up change can become stale.

### Security

Do not publish credentials, access tokens, private keys, sensitive logs, or vulnerability details in a public pull request.

For security-sensitive reports, follow the repository's [security policy](../SECURITY.md).
