# Local validation

> Added: 2026-08-27 09:10 JST  
> Last reviewed: 2026-08-27 11:38 JST

Use this reference when asking for or interpreting local checks in Tachyon.

## Rust toolchain selection

The Rust workspace pins its toolchain under `codex-rs/rust-toolchain.toml`. `rustup` selects a directory-scoped toolchain from the current working directory, so invoking Cargo from the repository root with only `--manifest-path codex-rs/Cargo.toml` is not guaranteed to select the pinned toolchain.

For Rust validation, prefer making `codex-rs` the current working directory first, then run commands such as:

```bash
cargo check -p codex-core
```

If toolchain selection looks wrong, verify:

```bash
rustc --version
rustup show active-toolchain
```

Do not solve an apparent MSRV failure by downgrading dependencies until the active toolchain has been verified.

## Formatting

`cargo fmt --all -- --check` may reveal formatting drift that already exists on `main`. A formatter failure is not automatically a regression from the current PR.

When unrelated files appear in formatter output:

1. identify whether the current PR touched them;
2. compare against the `main` baseline when necessary;
3. do not mix repository-wide formatting churn into an unrelated focused PR.

The workspace may emit a rustfmt warning for an unstable configuration such as `imports_granularity`; distinguish the warning from actual `Diff in ...` output.

## Warning policy

Do not casually suppress a new compiler warning introduced by the current change. First determine whether the warning exposes obsolete state or dead migration code that should now be removed.

A warning created by a cleanup PR can be evidence that the cleanup can proceed one step further.

## Source audits

GitHub code search is useful for navigation but is not trusted as an exhaustive use-site audit for this repository. When completeness matters, ask for a local recursive `grep` audit.

Do not assume `rg` is installed. Standard `grep` is the portable default unless the user has shown that another tool exists.

Prefer focused searches over large speculative edits, for example checking that removed APIs or intermediate states have zero remaining use sites before compiling.

For structural type/member migrations, do not make the exhaustive audit depend on the variable names or call shapes you expect to find. Searches such as `api_provider.base_url` or `provider().base_url` can miss the same member behind another identifier, helper, or method chain. Search the moved member/API itself workspace-wide, exclude generated/build directories such as `target`, and then classify each hit by the actual type/ownership context.

Use the compiler as a second safety net rather than as a substitute for the source audit. If compilation reveals a missed use site, broaden the audit pattern before rerunning so the migration converges by class of use site instead of one error at a time.

## Change history

- 2026-08-27 09:10 JST — Added Rust toolchain, formatter-baseline, warning, and source-audit guidance from Tachyon development experience.
- 2026-08-27 10:31 JST — Re-reviewed this reference and standardized freshness/change-history timestamps.
- 2026-08-27 11:38 JST — Added workspace-wide member/API audit guidance after provider field migration exposed identifier- and method-chain-specific grep blind spots.
