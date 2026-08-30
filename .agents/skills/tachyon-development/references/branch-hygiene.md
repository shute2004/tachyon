# Branch, Worktree, and Workspace Hygiene

Added: 2026-08-28 23:21 JST
Last reviewed: 2026-08-31 JST

## Goal

Keep Tachyon's active branches, worktrees, and local development directories small, legible, and intentionally owned across KE-AI and CDE-AI.

Temporary development state has a lifecycle. Creating a helper branch, detached validation worktree, or Desktop checkout also creates a cleanup responsibility.

## Creation rules

1. Before creating a branch or worktree, inspect current branches, open PRs, `git worktree list`, and the other track's state/worktree.
2. Reuse an existing safe worktree when it serves the same active unit. Do not create a fresh `tachyon-*` directory merely because it is convenient.
3. Every temporary branch/worktree should have an identifiable owner and purpose: active PR, reconstruction, focused validation, or paused unit.
4. If the reason a temporary branch/worktree must survive cannot be stated clearly, treat it as cleanup candidate rather than retained infrastructure.
5. Prefer roughly two simultaneous active implementation units across KE/CDE. Temporary validation/reconstruction worktrees are exceptions, not normal long-lived checkouts.
6. Never reuse or repurpose the other track's active worktree without coordination.

## Cleanup rules

1. After a PR merges or an implementation unit is abandoned, review every branch/worktree/local directory created for that unit before starting more temporary work.
2. Remove a completed temporary worktree once no follow-up, reconstruction, or unique uncommitted work depends on it.
3. Before removal, inspect worktree status. Never delete a dirty/untracked worktree just because its PR merged; first determine whether the local changes are disposable, already represented elsewhere, or must be preserved.
4. Cleanup verification must fail closed. If an existence check, `git status`, blob comparison, ancestry check, or other prerequisite command fails or returns incomplete data, classify the target as `KEEP / unresolved`; never let a default boolean or empty command result fall through to `SAFE TO REMOVE`.
5. For a registered Git worktree, use `git worktree remove <path>` rather than deleting the directory with `rm -rf`. Run `git worktree prune` after cleanup when stale registrations may remain.
6. Only remove a leftover non-worktree directory after verifying it is not the primary checkout, not an active/paused KE or CDE workspace, and contains no unique work that still matters.
7. Keep the primary Tachyon checkout and the active CDE upstream-sync worktree unless their ownership/state explicitly changes.
8. After cleanup, verify both `git worktree list` and the actual `~/Desktop/tachyon*` directories. Cleanup is not complete if abandoned directories remain on disk.
9. Pair branch cleanup with worktree cleanup. Deleting or pruning only one side while leaving the other as unexplained clutter is incomplete maintenance.
10. If a temporary branch/worktree/directory is accidentally created, clean it promptly instead of normalizing it as permanent workspace.

## Branch and GitHub rules

1. After a PR merges, delete its remote branch once no follow-up/reconstruction work depends on it.
2. Before deleting any branch, verify it is not referenced by an open PR, active/paused CDE or KE state, helper workflow, or local worktree that still needs it.
3. Treat GitHub as the authoritative repository state. Perform supported repository mutations on GitHub first, then update local repositories from GitHub with fetch/pull/prune.
4. Do not normally use local `git push` operations merely to make GitHub match local cleanup state.
5. For local synchronization after GitHub-side branch cleanup, prefer a short absolute-path command such as `cd /absolute/path && git fetch --prune origin` rather than manually deleting remote-tracking refs.
6. If the available GitHub connector cannot perform a required repository mutation such as deleting a branch ref, do not silently substitute a local push-based mutation. Leave that remote cleanup pending and state it explicitly.

## Periodic workspace audit

Do not wait for the Desktop root to become visibly cluttered before cleaning it.

Perform a workspace audit:

- after a cluster of PRs merges;
- before opening another wave of temporary validation worktrees;
- when several `tachyon-*` directories exist with unclear ownership;
- when storage pressure is being investigated;
- at handoff points between long development sessions.

The audit should classify each local Tachyon directory as one of:

- primary/long-lived and intentionally retained;
- active KE work;
- active CDE work;
- paused but explicitly retained;
- safe cleanup candidate.

Cleanup candidates should be removed in the same maintenance cycle after status verification.

## Current responsibility

- KE-AI cleans up branches, validation worktrees, and helper directories it creates for completed KE units.
- CDE-AI cleans up completed CDE branches/worktrees and its helper directories after their dependent unit is finished.
- Neither track deletes the other track's active or paused branch/worktree without checking its state.
- The agent creating temporary workspace state is responsible for remembering the corresponding cleanup condition; the user should not have to discover accumulated leftovers later.

## Change history

- 2026-08-28 23:21 JST — Added branch hygiene guidance.
- 2026-08-28 23:29 JST — Made GitHub authoritative for supported branch cleanup mutations.
- 2026-08-29 12:12 JST — Expanded the canonical hygiene guidance to cover temporary worktree/local-directory lifecycle, periodic Desktop workspace audits, and creator-owned cleanup responsibility.
- 2026-08-29 12:12 JST — Added fail-closed cleanup verification after a missing worktree caused a shell probe to print `SAFE TO REMOVE` despite prerequisite `git` failures.
- 2026-08-31 JST — Moved the canonical project-specific guidance from Second-Brain into the Tachyon development Agent Skill.