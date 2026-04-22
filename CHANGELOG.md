# Changelog

All notable user-facing changes to this project are recorded here.

The format follows the `### Features / ### Changes / ### Fixes` structure expected by
the changelog-manager agent (see `.claude/agents/changelog-manager.md`). This project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## How entries are added

New entries land via the `post-pr-create-changelog` hook (see `.claude/hooks/`).
The unreleased version for any in-flight PR is resolved from the PR's GitHub
milestone, so make sure your PR is assigned to the correct milestone before
opening it.

Entry style:

- One past-tense imperative line per change. Start with "Added", "Changed", "Fixed", or "Removed".
- Use backticks for code identifiers (`fetch_notes`, `StoredNote`, `seq`).
- Prefix breaking changes with `[BREAKING] `.
- End with the PR link in parentheses, then a period.

Example:

```
- Fixed `fetch_notes` pagination race by introducing a monotonic `seq` cursor ([#77](https://github.com/0xMiden/note-transport-service/pull/77)).
- [BREAKING] Removed deprecated `fetch_notes_legacy` ([#82](https://github.com/0xMiden/note-transport-service/pull/82)).
```

Skip the changelog only when the PR contains no runtime-affecting changes
(docs, CI, tooling, tests). In that case the hook will tell you to apply the
`no changelog` label instead.

## v0.4.0 (unreleased)

### Features

### Changes

### Fixes

## v0.3.1 (2026-04-08)

Released before this changelog was started. See [`git log v0.3.0..v0.3.1`](https://github.com/0xMiden/note-transport-service/compare/v0.3.0...v0.3.1) for the change set.

## v0.3.0 (2026-04-08)

Released before this changelog was started. See [`git log v0.2..v0.3.0`](https://github.com/0xMiden/note-transport-service/compare/v0.2...v0.3.0) for the change set.

## v0.2 (2026-01-24)

First tagged release. Earlier history available via `git log v0.2`.