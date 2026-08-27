# Terminal and patching workflow

> Added: 2026-08-27  
> Last reviewed: 2026-08-27

Use this reference before giving interactive shell commands or constructing a local patch workflow.

## Interactive shell safety

Do not enable `set -e` in the user's interactive Zed terminal for this workflow.

A failing command under shell errexit can terminate the interactive shell. In Zed this can look like the terminal suddenly disappeared. Prefer short commands executed one at a time and let the user stop at the first unexpected result.

If errexit may already be active, `set +e` can disable it before continuing.

## Do not manufacture unnecessary directory changes

Read the shell prompt the user provides. If the current directory is already clear and correct, do not prepend a redundant `cd` command.

An exception is when changing directories has semantic effect, such as entering `codex-rs` so `rustup` discovers the repository-pinned toolchain. Explain that reason when the directory change matters.

## Patching

Avoid large Python heredocs pasted directly into an interactive terminal when a smaller deterministic approach is available. A long heredoc is hard to inspect, easy to break, and interacts badly with shell-level failure behavior.

Prefer, in order:

1. a small normal source edit through the repository workflow;
2. a short deterministic helper file staged on a temporary branch when the user must apply a structured local transformation;
3. small commands whose result can be inspected after each step.

If a generated patch fails `git apply --check`, treat the patch as suspect before blaming the user's checkout. Do not continue to `git apply` after a failed check.

Temporary helper files must not survive into the final PR diff. If helper commits were used only to transport a local edit, rewrite the short-lived branch onto `main` before the final commit so the PR history and tree stay clean.

## Destructive Git operations

Do not use `git reset --hard` casually. Use it only when the workflow has established that local uncommitted work does not need preservation, or when the user has explicitly accepted that reset.

When rewriting a remote short-lived branch intentionally, prefer `git push --force-with-lease` rather than an unconditional force push.

## Change history

- 2026-08-27 — Added Zed interactive-shell safety, working-directory semantics, deterministic patching, and Git rewrite guidance from Tachyon development experience.
