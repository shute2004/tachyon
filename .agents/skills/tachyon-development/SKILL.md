---
name: tachyon-development
description: Use when developing, refactoring, validating, reviewing, or merging changes in the Tachyon coding harness kernel.
---

# Tachyon Development

> Added: 2026-08-27 09:10 JST  
> Last reviewed: 2026-08-31 JST

Use this skill for work in `shute2004/tachyon`.

## Core workflow

1. Read the current `main`, `TACHYON.md`, `docs/tachyon/architecture.md`, and the implementation directly relevant to the change. Do not design from PR text or issue text alone.
2. Preserve behavior first. Move dependency direction and ownership boundaries incrementally rather than rewriting mature Codex behavior from scratch.
3. Before expanding a change beyond its immediate responsibility, read `references/implementation-scope-discipline.md` and distinguish a true prerequisite from an adjacent improvement.
4. Use a short-lived focused branch from current `main`. For branch/worktree lifecycle and cleanup, read `references/branch-hygiene.md`.
5. Before editing model-runtime or other provider-boundary code, distinguish generic harness capability from provider/protocol-specific realization. Keep concrete provider mechanisms below adapters unless their generic semantics are demonstrated. For the detailed extraction rules, read `references/architecture-extraction.md`.
6. For local Rust checks, read `references/local-validation.md`. When several local checks or ordered steps must be handed to the user, also read `references/local-validation-command-handoff.md` and minimize user operations by packaging validation when practical.
7. Before sending terminal commands or constructing patching workflows, read `references/terminal-and-patching.md`.
8. For branch, PR, and merge workflow, read `references/git-pr-workflow.md`. For durable architecture-boundary changes, also read `references/independent-architecture-review.md`.
9. Check the freshness metadata and change history of any Skill/reference guidance you rely on. If it is old relative to current code, verify the relevant implementation before treating it as authoritative.
10. Project-specific reusable developer-agent guidance belongs in this Tachyon Agent Skill. Mutable progress/current SHAs, coordination, and historical snapshots belong in the private development-memory repository rather than being duplicated as a second active Skill source.

## Upstream Codex synchronization

Upstream Codex synchronization is part of normal Tachyon maintenance, not an occasional catch-up task.

Run an upstream audit whenever either condition is met, whichever happens first:

- one Tachyon architecture PR is merged; or
- the unaudited `openai/codex` upstream delta reaches roughly 15 commits.

Classify upstream commits as `Direct port`, `Adapt port`, `Defer`, or `Ignore`. The goal is to keep the number of unaudited quality-relevant upstream commits near zero, not to mechanically mirror product/UI-only changes.

Keep extraction work, upstream synchronization, and Tachyon-specific product or harness improvements as separate development concerns and preferably separate PRs. When an upstream change touches a Tachyon-diverged architecture boundary, preserve the upstream behavioral improvement but adapt it to the established Tachyon boundary instead of blindly cherry-picking the upstream structure.

Track the last audited upstream commit separately as mutable project progress rather than hard-coding it into this Skill.

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

## Change history

- 2026-08-27 09:10 JST — Added the initial Tachyon development workflow and established current ModelRuntime/ModelTurnRuntime invariants.
- 2026-08-27 10:05 JST — Added explicit Skill freshness checks to the core workflow.
- 2026-08-27 10:31 JST — Re-reviewed the full Skill and standardized freshness/change-history timestamps.
- 2026-08-27 13:46 JST — Added high-frequency Codex upstream audit cadence and separation between extraction, upstream synchronization, and Tachyon-specific improvements.
- 2026-08-31 JST — Consolidated project-specific branch hygiene, scope discipline, independent review, and low-burden local-validation handoff guidance from Second-Brain into this Agent Skill.