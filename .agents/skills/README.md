# Tachyon Agent Skills

> Added: 2026-08-27 09:10 JST  
> Last reviewed: 2026-08-27 10:31 JST

This directory contains project-specific Agent Skills for work in `shute2004/tachyon`.

| Skill | Purpose |
| --- | --- |
| [`tachyon-development`](tachyon-development/SKILL.md) | Guides Tachyon development, architectural extraction, local validation, terminal safety, Git/PR workflow, and preservation of generic harness capabilities while provider-specific implementation is moved behind adapters. |

Reusable skills that are not Tachyon-specific belong in the private `shute2004/skills` repository instead of being duplicated here.

## Freshness metadata

Skill guidance is operational knowledge, not timeless documentation. Every `SKILL.md` and reference Markdown file should make its age visible with:

```text
Added: YYYY-MM-DD HH:MM JST
Last reviewed: YYYY-MM-DD HH:MM JST
```

Also keep a concise `Change history` that dates meaningful additions or revalidation using the same timestamp format:

```text
- YYYY-MM-DD HH:MM JST — Description of the change or review.
```

This lets an agent distinguish an old but still-reviewed rule from guidance that has not been checked against the current repository for a long time.

Rules:

- `Added` records when the file first became active guidance.
- `Last reviewed` is updated only when the file's guidance has actually been checked against the current project state, not merely because one line was edited.
- When new operational knowledge is appended to an existing file, add a timestamped `Change history` entry describing the addition.
- If only one section is revalidated, record that scope in `Change history`; do not imply that unrelated old sections were reviewed.
- Use `YYYY-MM-DD HH:MM JST` consistently. Do not mix date-only, Japanese calendar-style, UTC, or unlabeled local timestamps in these files.
- Stale metadata is a signal to verify the relevant code and current architecture before relying on the guidance.

## Change history

- 2026-08-27 09:10 JST — Added the Tachyon Skill catalog and repository placement rule.
- 2026-08-27 10:05 JST — Added explicit freshness metadata and dated change-history conventions for Agent Skills.
- 2026-08-27 10:31 JST — Standardized all Skill freshness and change-history timestamps on `YYYY-MM-DD HH:MM JST` and re-reviewed this catalog.
