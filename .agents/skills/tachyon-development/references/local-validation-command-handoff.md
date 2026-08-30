# Local Validation Command Handoff

> Added: 2026-08-28 JST
> Updated: 2026-08-31 JST

## Purpose

KE-AI does not assume direct shell access to the user's Mac. Local validation must therefore be designed to minimize the user's execution and transcription burden, not merely to make commands individually pasteable.

The user should not have to remember which of several similar commands have already run, repeatedly copy raw terminal output back into chat, or manually coordinate a long validation sequence when that sequence can be packaged by KE-AI.

## Known local layout

Primary Tachyon checkout:

`/Users/kumamotoshuto/Desktop/tachyon`

KE-AI and CDE-AI use separate tracks/worktrees. Do not enter, reset, clean, rebase, or otherwise modify a CDE-owned worktree during KE validation.

## Handoff rules

1. Minimize **user operations**, not only command-block count. A single long pasted block can still impose high cognitive load; splitting it into several blocks can be worse.
2. If validation requires several tests/checks, repeated environment variables, multiple directories, or ordered steps, prefer creating a checked-in executable validation script on the relevant branch. The user should normally need to fetch the branch and invoke one script once.
3. The script should verify the expected repository/worktree and exact head before expensive work, then run the full intended validation sequence itself.
4. The script should collect exit status for each check and produce one concise machine-readable or Markdown summary. Preserve detailed logs locally only when useful for diagnosing a failure.
5. When practical and safe, prefer validation that can be observed directly through GitHub CI. For genuinely local-only validation, prefer a workflow in which the generated summary can be made available through GitHub with minimal user interaction. Do not require the user to paste several raw command outputs merely because that was the historical workflow.
6. Do not automatically commit or push local validation artifacts unless the validation workflow was deliberately designed for that branch and doing so cannot contaminate production history. If GitHub-visible local results are needed, use a dedicated validation-result path/branch or another explicit mechanism rather than silently modifying the implementation branch.
7. For a genuinely tiny check consisting of one short command, direct command handoff remains acceptable. Do not create a helper script merely for ceremony.
8. Every direct command handoff must begin with an explicit absolute `cd` to the intended KE directory/worktree. Do not rely on the user's current terminal directory.
9. Pin validation to the exact PR head when possible. A detached temporary worktree is acceptable and avoids disturbing the primary checkout or concurrent branches.
10. Enter `codex-rs` before Cargo commands so the repository-pinned Rust toolchain is selected.
11. Use source audit plus compiler/tests; do not substitute one for the other.
12. Because Tachyon builds have previously exhausted local disk, default heavy checks to reduced artifact pressure and parallelism when appropriate:

```bash
CARGO_INCREMENTAL=0 \
CARGO_PROFILE_DEV_DEBUG=0 \
CARGO_PROFILE_TEST_DEBUG=0 \
cargo check -j 2 ...
```

Use the same `-j 2` discipline for heavy tests unless there is a reason not to.
13. Storage pressure is a recurring maintenance concern. Inspect free space when sustained Cargo work makes it relevant. Do not run `cargo clean` blindly; only clean at a safe boundary after confirming no build/test is active.
14. If `No space left on device (os error 28)` occurs, classify it as an environment/storage failure first.
15. Do not claim local validation passed until the actual result has been received or inspected. A script-generated summary is valid evidence; several manually pasted raw outputs are not required.
16. Do not place executable local commands in intermediary progress messages. Finalize the intended validation method first so the user never has to track superseded command sequences.

## Preferred validation packaging

For a non-trivial PR, prefer this shape:

```text
GitHub branch
  └─ scripts/validate-<scope>.sh
       ├─ verifies exact head/worktree
       ├─ runs all focused tests
       ├─ runs compile/static checks
       ├─ records PASS/FAIL per step
       └─ emits one concise summary

User action
  └─ fetch/pull + execute the script once
```

If the same checks can run reliably in GitHub Actions, prefer CI and inspect the result directly instead of delegating execution to the user.

## Baseline KE validation

For a small `codex-core` architecture slice, the underlying checks may still include `cargo check -p codex-core`, focused tests, formatting, and `git diff --check`. The important distinction is that when several of these are required together, KE-AI should package them rather than making the user manually orchestrate them.

## Historical evidence

Earlier Part2 KE work used direct command handoff and required repeated terminal-output transcription. This worked technically but imposed unnecessary cognitive load as validation sequences grew. The current rule is therefore operation-oriented: keep the user's required actions minimal and package multi-step validation whenever practical.

## Change history

- 2026-08-31 JST — Moved the canonical project-specific workflow from Second-Brain into the Tachyon development Agent Skill.