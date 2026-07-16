# Project Lessons - note-transport-service

> Codified mistakes, conventions, and patterns surfaced during real PR reviews.
> Claude reads this at the start of every non-trivial task (see CLAUDE.md).
>
> Add a new entry with:
>
>     /codify-lesson <one-line description>
>
> Each entry must commit to a promotion path:
>
>   - **Keep as lesson**: judgment call, context-dependent, hard to mechanize.
>   - **Promote to hook**: mechanically enforceable. Lives under `.claude/hooks/`.
>   - **Promote to CLAUDE.md**: global project rule that applies to all tasks.
>
> A lesson promoted to a hook or CLAUDE.md should be removed from this file
> with a note like `(promoted to .claude/hooks/foo.sh on YYYY-MM-DD)` so the
> file stays a list of active soft rules, not a graveyard.

## Conventions
_Naming, formatting, vocabulary, file layout, doc style._

_(no entries yet)_

## Architecture
_Module boundaries, abstraction choices, API design, dependency direction._

_(no entries yet)_

## Testing
_What to test, how to test, fixtures, regression patterns, coverage gaps._

- **No cross-version compatibility guarantees** (2026-07-16): the chain is
  pre-mainnet and will be wiped, so stored data and wire formats do not need
  to survive protocol bumps. Don't add version-pinned serialization fixtures,
  migration paths, or compat tests when bumping miden-protocol - and push back
  when an automated reviewer asks for them. **Keep as lesson.**

## Security & Safety
_Validation, auth, data handling, error paths, panics, resource limits._

_(no entries yet)_

## Process
_Workflow, commits, PRs, CI, review etiquette, branch naming._

- **Never chain `git commit` with `git push` in one command** (2026-07-16): the
  pre-push review hook fires before the Bash command runs and blocks the whole
  command, so a blocked `git commit && git push` never executes the commit
  either. A later `--amend` then rewrites the wrong commit. Commit and push in
  separate commands, and after any blocked push re-check `git log` before
  amending. Note the hook's `SKIP_PRE_PUSH=1` escape hatch cannot be triggered
  from within a Claude session (the env prefix never reaches the hook process);
  only the user can bypass, from their own terminal. **Keep as lesson.**
