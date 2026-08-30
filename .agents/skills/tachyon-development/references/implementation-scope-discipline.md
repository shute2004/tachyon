# Implementation Scope Discipline

Purpose: prevent KE-AI/CDE-AI from over-engineering Tachyon changes and keep each implementation focused on the behavior and architecture actually required by the current slice.

## Core rule

Before adding an abstraction, enum variant, helper layer, generalized API, compatibility mechanism, test matrix, or documentation surface, ask:

> Is this required to preserve current behavior, satisfy the current architecture boundary, fix an observed defect, or validate the exact change being made now?

If the answer is no, defer it.

Do not implement something merely because it may be useful later, would make the design more general, or would complete a theoretically cleaner abstraction.

## Focused slices are not a minimal kernel

The rule above constrains the size of each implementation slice. It does **not** mean Tachyon itself should be reduced to a small or minimal SDK.

Tachyon's Kernel is the reusable coding-agent runtime extracted from Codex: session/thread/turn lifecycle, agent loop, context/history/compaction, tool execution, sandboxing, permissions/approvals, persistence/resume/fork/rollback, MCP/extensions, recovery, and other mature harness behavior can all belong in the Kernel when their semantics are model/provider/UI-independent.

A feature being sophisticated, optimized, or currently implemented with OpenAI/Codex-specific machinery is not a reason to remove it. Distinguish:

1. pure provider/product/UI detail — keep outside the Kernel or behind the relevant adapter;
2. provider-specific realization of a general harness capability — preserve the capability and move the concrete realization behind a boundary;
3. behavior that exists only to optimize or support one provider/product and has no reusable harness semantic — this may be removed from the Kernel when doing so does not degrade the general coding-agent runtime.

Do not confuse "remove unnecessary implementation from this PR" with "remove useful capability from Tachyon". The target is a feature-rich, reusable coding-agent runtime that becomes a complete harness when connected to a host/UI, not an OS-style minimal kernel.

When deciding whether Codex-derived behavior can be deleted, first identify what harness capability it provides and verify that the capability is either preserved elsewhere in the Kernel/adapter architecture or genuinely unnecessary for a model-agnostic coding harness.

## Required practice

1. Prefer the smallest implementation that correctly satisfies the current work item.
2. Keep focused PRs focused. Do not absorb adjacent cleanup, redesign, or future migration work unless it is a real prerequisite.
3. Distinguish a prerequisite from an improvement:
   - prerequisite: the current change cannot be correct, compile, preserve behavior, or satisfy its architecture contract without it;
   - improvement: the current change already works and the addition mainly makes future work easier or the design more elegant.
   Only prerequisites belong in the current slice by default.
4. Reuse an existing seam when it is sufficient. Do not create a new abstraction just to avoid a small amount of local adapter code.
5. Do not genericize provider-specific behavior without evidence that the harness itself needs the semantic capability.
6. Conversely, when a genuine harness capability is identified, represent only the semantic nucleus required by current behavior; do not pre-design every likely future provider variant.
7. Preserve explicit legacy/fallback paths when that is cheaper and safer than broadening the generic model prematurely.
8. Tests should prove the behavior and boundary changed by the slice, plus important fallback/regression behavior. Avoid speculative combinatorial coverage unrelated to the current change.
9. Documentation should record stable architectural truth introduced by the slice. Do not document hypothetical future architecture as if it already exists.
10. If a proposed addition materially increases changed files, API surface, branch dependencies, or review burden, require a concrete reason tied to the current slice before including it.

## Scope check before committing

Ask all of the following:

- What exact user-visible, harness-visible, or architecture behavior requires this code?
- Would removing this piece make the current slice incorrect or incomplete?
- Is this solving an observed current problem, or a guessed future problem?
- Can the same correctness be achieved with a smaller diff or by keeping the existing fallback?
- Am I expanding the abstraction because Tachyon needs it now, or because a more general design feels cleaner?
- Am I accidentally deleting or weakening a mature harness capability merely because its current implementation is provider-specific?

If a piece fails these checks, remove or defer it.

## Review signal

During self-review, explicitly look for over-engineering as a defect category. Typical signals:

- abstraction introduced with only one current caller and no boundary need;
- generic type carrying provider-specific concepts for possible future use;
- broad refactor bundled into a narrow migration or repair;
- replacement of a safe fallback with a large generalized implementation;
- support for hypothetical states that current code cannot produce;
- excessive helper layers, indirection, configuration, or extension points without a current requirement;
- tests or docs growing substantially beyond the semantic scope of the change.

Also look for the opposite failure mode: over-pruning. Typical signals:

- deleting capability because the concrete implementation is OpenAI/Codex-specific without first identifying the general harness semantic;
- treating a feature-rich runtime concern as product bloat merely because an OS kernel analogy suggests minimalism;
- moving behavior out of the Kernel without a provider-neutral host/kernel or adapter replacement;
- preserving a thin API while silently losing mature retry, recovery, context, tool, sandbox, permission, persistence, or lifecycle behavior.

When in doubt, prefer a small migration slice while preserving the mature capability behind an existing adapter/fallback until its correct ownership is clear.

## Change history

- 2026-08-31 JST — Moved the canonical project-specific guidance from Second-Brain into the Tachyon development Agent Skill.