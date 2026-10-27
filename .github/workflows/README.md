# Workflow Strategy

The workflows in this directory are split so that pull requests get fast, review-friendly signal while `main` still gets the full cross-platform verification pass.

## Pull Requests

- Required checks run against GitHub's synthetic merge commit, not the pull
  request head alone. This includes changes already on `main` and catches
  conflicts before they reach the branch.
- `bazel.yml` is the main pre-merge verification path for Rust code.
  It runs Bazel `test` and Bazel `clippy` on the supported Bazel targets,
  including the generated Rust test binaries needed to lint inline `#[cfg(test)]`
  code.
- `rust-ci.yml` keeps the Cargo-native PR checks intentionally small:
  - `cargo fmt --check`
  - `cargo shear`
  - `argument-comment-lint` on Linux, macOS, and Windows
  - `tools/argument-comment-lint` package tests when the lint or its workflow wiring changes

## Post-Merge On `main`

- `bazel.yml` also runs on pushes to `main`.
  This re-verifies the merged Bazel path and helps keep the BuildBuddy caches warm.
- `rust-ci-full.yml` is the full Cargo-native verification workflow.
  It keeps the heavier checks off the PR path while still validating them after merge:
  - the full Cargo `clippy` matrix
  - the full Cargo `nextest` matrix via per-platform archive-backed shards
  - Windows ARM64 nextest archives cross-compiled on Windows x64, then replayed on native Windows ARM64 shards
  - release-profile Cargo builds
  - cross-platform `argument-comment-lint`
  - Linux remote-env tests

## Rule Of Thumb

- If a build/test/clippy check can be expressed in Bazel, prefer putting the PR-time version in `bazel.yml`.
- Keep `rust-ci.yml` fast enough that it usually does not dominate PR latency.
- Reserve `rust-ci-full.yml` for heavyweight Cargo-native coverage that Bazel does not replace yet.

## Build Reuse And Performance Evidence

- `blocking-ci.yml` runs inexpensive repository, spelling, blob-size, and
  dependency-policy checks before starting the build matrix. New PR revisions
  cancel obsolete runs; required checks still reject failures and cancellations.
- Without BuildBuddy credentials, public runners use local Bazel execution.
  Repository caches reuse downloads, not compiled outputs. The SDK and PR
  argument-comment lint also opt into separate compiled-action caches.
- SDK action-cache uploads are capped at 4 GiB; lint uploads at 2 GiB per
  platform. Cache-service errors are nonfatal and do not change the build result.
  Lint cache keys separate OS, architecture, target, toolchain/configuration,
  and revision; fork PRs may restore but do not upload lint caches.
- Keyless Windows cross-builds use matching GNU-ABI host and target toolchains
  to avoid spending a build on incompatible MSVC/GNU link inputs. Explicit
  MSVC-host requests and authenticated remote execution retain their existing
  paths; wrapper unit tests do not replace a real Windows build.
- Compare cold and warm runs using the same runner, configuration, and actual
  target workload. Record the exact run/revision, cache hit, build-step duration,
  and Bazel execution logs. A successful cache-restore step is not evidence of a
  hit, and a skipped build is not a successful validation.
- These caches reduce repeat work after a usable cache has been saved. They do
  not by themselves fix a cold build that exceeds its job timeout: cancelled
  jobs do not reach the explicit save steps. Cold-run completion and measured
  warm-run speedups must be verified separately.
