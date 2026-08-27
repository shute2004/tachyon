# Local validation

> Added: 2026-08-27 09:10 JST  
> Last reviewed: 2026-08-28 04:05 JST

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

## Worktree identity before heavy validation

Tachyon development may use multiple local worktrees for parallel tracks such as Architecture and Upstream Sync. Before a heavy `cargo check` or `cargo test`, verify that the terminal is inside the intended worktree and branch rather than relying on the prompt or a previous `cd`.

Prefer a lightweight preflight such as:

```bash
pwd
git status --short --branch
```

If a dedicated worktree is part of the current track's handoff or Skill guidance, confirm that the reported path matches it before starting the heavy Cargo command. A command accidentally run in another valid Tachyon worktree is an execution-location mistake; do not infer that the worktree separation design itself is invalid.

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

## Build artifact cleanup

The `codex-rs/target` directory can grow rapidly during repeated workspace checks, branch changes, and upstream-sync validation.

At safe boundaries—after a build/test command has completed, between focused validation batches, or when disk pressure becomes material—check build-artifact size and available disk space. Prefer:

```bash
du -sh target 2>/dev/null
df -h .
cargo clean
```

Do not run `cargo clean` while another Cargo build or test is active.

A `No space left on device (os error 28)` failure should first be treated as an environment/storage failure, not as evidence that the current code change is invalid. Clean generated artifacts, confirm free space, then rerun the interrupted validation.

When free space is tight, avoid unnecessarily multiplying build artifacts across validation variants. It is acceptable to disable incremental compilation and debug information for correctness-oriented local checks when those settings are not what the change is testing, for example with `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, and `CARGO_PROFILE_TEST_DEBUG=0`.

Heavy `codex-app-server`, `codex-core`, and broad integration validations can have a much larger transient disk peak than the final `target/` size because multiple `rustc` jobs create `.rmeta`, object, and archive files concurrently. Do not use the current `target/` size alone to decide whether a heavy validation is safe. On a constrained local disk, treat roughly 30 GiB of free space as a warning threshold rather than a guarantee. After a prior ENOSPC failure, clean at a safe boundary before the next heavy validation and cap Cargo parallelism, for example:

```bash
CARGO_INCREMENTAL=0 \
CARGO_PROFILE_DEV_DEBUG=0 \
CARGO_PROFILE_TEST_DEBUG=0 \
cargo check -j 2 -p codex-app-server
```

Use the same `-j 2` limit for heavy `cargo test` commands when disk peak is the concern. If a clean target plus reduced parallelism still cannot fit locally, prefer CI or a larger build volume for that heavy integration check instead of repeatedly retrying a high-parallel local build.

## Change history

- 2026-08-27 09:10 JST — Added Rust toolchain, formatter-baseline, warning, and source-audit guidance from Tachyon development experience.
- 2026-08-27 10:31 JST — Re-reviewed this reference and standardized freshness/change-history timestamps.
- 2026-08-27 11:38 JST — Added workspace-wide member/API audit guidance after provider field migration exposed identifier- and method-chain-specific grep blind spots.
- 2026-08-27 13:46 JST — Added build-artifact cleanup and low-disk validation guidance after repeated Rust builds exhausted local storage.
- 2026-08-27 14:12 JST — Added transient build-peak and reduced-parallelism guidance after an app-server validation exhausted more than 23 GiB of free space despite incremental/debug reductions.
- 2026-08-28 04:05 JST — Added multi-worktree identity preflight guidance before heavy Cargo validation.
