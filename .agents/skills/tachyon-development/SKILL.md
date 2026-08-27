---
name: tachyon-development
description: Use when developing, refactoring, validating, reviewing, or merging changes in the Tachyon coding harness kernel.
---

# Tachyon Development

Use this skill for work in `shute2004/tachyon`.

## Core workflow

1. Read the current `main`, `TACHYON.md`, `docs/tachyon/architecture.md`, and the implementation directly relevant to the change. Do not design from PR text or issue text alone.
2. Preserve behavior first. Move dependency direction and ownership boundaries incrementally rather than rewriting mature Codex behavior from scratch.
3. Use a short-lived focused branch from current `main` and keep the PR scoped to one architectural or behavioral responsibility.
4. Before editing model-runtime code, distinguish generic harness capability from provider/protocol-specific realization. Keep concrete OpenAI/Codex mechanisms below adapters unless their generic semantics are demonstrated.
5. For local Rust checks, read `references/local-validation.md`.
6. Before sending terminal commands or patching workflows, read `references/terminal-and-patching.md`.
7. For branch, PR, merge, and independent-review workflow, read `references/git-pr-workflow.md`.
8. For ModelRuntime, Provider/Protocol/Route, Endpoint/Auth, compaction, retry, or provider-private state work, read `references/architecture-extraction.md`.
9. If development reveals a non-obvious rule that a fresh agent would likely miss, consider updating this skill or recording a candidate in Notion. Do not add skill content mechanically after every task.

## Architectural invariants currently established

- `ModelRuntime` is session-scoped.
- `ModelTurnRuntime` is fresh per harness turn and execution-capable turn runtimes are provider-bound.
- A turn-scoped `ModelRoute` currently represents provider identity, protocol identity, and transport; Tachyon does not keep a provider-less partial route.
- Fresh turn-affinity state must not leak across turns. Reusable provider-private backend state may cross turns behind the adapter.
- Existing `ModelClient` / `ModelClientSession` behavior remains a regression oracle during extraction, not the target generic API.

## Completion standard

A migration PR should make clear:

- what behavior intentionally stays unchanged;
- what ownership or dependency direction changed;
- which provider-specific mechanisms remain behind the adapter;
- what is deliberately deferred;
- which focused checks were run and whether any warning or baseline failure remains.
