# Independent Architecture Review Workflow

> Added: 2026-08-28 JST
> Last reviewed: 2026-08-31 JST

Use this workflow for Tachyon PRs that change a durable architecture boundary, lifetime, ownership model, or the generic/provider-specific split.

## Reviewer

The formal independent reviewer is a separate LLM reviewer operating in the same ChatGPT project with GitHub access. GitHub Copilot review may be treated as supplemental signal only; it does not satisfy Tachyon's formal independent architecture-review gate.

KE-AI must not self-certify this gate.

## Handoff procedure

Before merge, KE-AI prepares a reviewer prompt for the user to send to the independent reviewer. The prompt must identify:

- repository and PR number
- exact PR head SHA
- current `main`
- relevant architecture documents and Agent Skill guidance to inspect
- relevant implementation files that must be read rather than relying on the PR body
- the specific architecture questions introduced by the PR
- explicit instruction not to modify code, branches, PR metadata, or repository state
- required verdict structure: `Approve`, `Conditional Approve`, or `Request Changes`, with findings classified as `Blocker`, `High`, `Low`, or `Nit`

The reviewer should inspect the actual GitHub diff and current code independently. The PR description is context, not evidence.

If the PR head changes after review, the review is stale and a new exact-head review is required.

## Merge gate

A boundary-changing PR may merge only after:

1. exact-head source/CI validation is acceptable;
2. the independent project reviewer has returned an architecture verdict for that exact head;
3. all `Blocker` and `High` findings are resolved or explicitly re-reviewed;
4. current `main`, concurrent CDE work, and changed-file overlap are rechecked immediately before merge.

`Conditional Approve` is mergeable only when every stated condition is demonstrably satisfied without requiring another architecture judgment. Otherwise request a follow-up review.

## Review-prompt style

Do not prime the reviewer toward approval. State the intended boundary and invariants, but ask the reviewer to actively look for incorrect abstraction, behavior regressions, provider-specific leakage, lost generic capabilities, lifetime/state errors, and insufficient tests.

## Change history

- 2026-08-28 — Replaced GitHub Copilot as the formal Tachyon architecture-review gate with a separate project LLM reviewer that independently reads GitHub state. Copilot remains supplemental only.
- 2026-08-31 — Moved the canonical project-specific workflow from Second-Brain into the Tachyon development Agent Skill.