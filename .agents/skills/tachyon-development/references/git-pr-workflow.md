# Git and pull-request workflow

Tachyon uses a trunk-based / GitHub-Flow-like workflow.

## Branch model

`main` is the only normal long-lived development branch.

Create one short-lived branch for one focused change, for example:

- `refactor/model-endpoint-boundary`
- `refactor/model-auth-boundary`
- `fix/startup-runtime-transfer`
- `docs/model-runtime-lifetime`
- `test/provider-route-invariants`

Do not use branches as historical archives. Git history, merged PRs, and release tags preserve history.

After a PR is merged, delete its head branch unless it is an explicitly documented long-running experiment or release branch.

## PR scope

Prefer one architectural responsibility per PR. State:

- what changes;
- what behavior is intentionally unchanged;
- what is out of scope;
- how the change was validated.

Do not mix unrelated cleanup, formatting churn, or Skill maintenance into an architectural migration PR.

## Review level

Use independent architecture review when a PR changes a durable boundary, invariant, lifetime, ownership model, or generic/provider-specific split.

Mechanical moves, renames, obvious call-site migrations, and documentation-only changes do not need redundant independent architecture review unless they reveal a boundary change.

A reviewer should inspect actual current code, `TACHYON.md`, `docs/tachyon/architecture.md`, and relevant Codex-derived implementation rather than trusting the PR description alone.

Use a verdict structure such as `Approve / Conditional Approve / Request Changes` with `Blocker / High / Low / Nit` findings.

## Merge

Prefer squash merge for focused migration PRs so `main` remains readable even if the short-lived branch temporarily contained helper or correction commits.

Treat branch deletion as part of PR completion.

## Repository-specific constraint

GitHub Issues are currently disabled for `shute2004/tachyon`. Do not fail the workflow merely because an Issue cannot be created. In that case, use the branch and PR body as the explicit scope record.
