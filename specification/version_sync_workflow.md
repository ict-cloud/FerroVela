# Version bump → dev sync workflow

## Purpose

Version bumps land on `master` (via the "Bump Patch Version" PR created by
`bump-version.yaml`). `dev` does not receive that commit automatically, so the
two branches drift apart. The `sync-dev-after-version-bump.yaml` workflow
closes that gap.

## Flow

1. A PR is merged into `master`.
2. `bump-version.yaml` opens `chore/bump-version-to-X.Y.Z` against `master`;
   merging it pushes the bump commit to `master`.
3. `tag.yaml` tags `vX.Y.Z` (unchanged).
4. `sync-dev-after-version-bump.yaml` runs on that push:
   - Compares the `version` in `Cargo.toml` between `github.event.before` and
     `github.sha`. If unchanged, the job stops without touching `dev`.
   - If changed, fetches `dev` and checks whether it already contains the new
     `master` head. If so, it stops.
   - Otherwise it builds `sync/master-X.Y.Z` from `dev`, merges the `master`
     commit into it, and force-pushes that branch (it is bot-owned).
   - It opens a PR `sync/master-X.Y.Z` → `dev` (skipped if one is already open)
     and enables auto-merge.

`dev` is protected ("changes must be made through a pull request"), so the
workflow never pushes to `dev` directly. The PR merges once the branch rules
are met (required checks and approvals). When `dev` has no commits of its own,
the merge is a fast-forward, so the PR is just the bump commit.

## Behaviour and limits

- **Trigger scope**: only pushes to `master`. Pushes to `dev` do not start it,
  so the workflow cannot loop.
- **Concurrency**: runs are serialized (`cancel-in-progress: false`) so a
  merge is never cancelled half-way.
- **Conflicts**: if the merge conflicts (for example, `dev` also edited the
  `version` line or `Cargo.lock`), the job fails and nothing is pushed. Resolve
  the merge on `dev` manually.
- **Auto-merge** requires "Allow auto-merge" in the repository settings. If it
  is off, the job still succeeds, emits a warning, and the PR must be merged by
  hand.
- **Approvals**: if `dev` requires an approving review, auto-merge waits for
  one. The bot does not approve its own PR.
- **Triggering**: PRs opened by `GITHUB_TOKEN` do not start other workflows
  except `pull_request` ones; `test.yaml` runs on PRs into `dev`, so this is
  covered as long as that holds for your repository.
- **Cargo.lock**: `bump-version.yaml` edits only `Cargo.toml`. The lock file's
  workspace entries are refreshed on the next `cargo build` and are not part of
  this sync.

## Testing

- YAML parses (`python3 -c "import yaml; yaml.safe_load(open(...))"`).
- Version detection and merge were simulated in a scratch repository: a bump
  from `0.4.7` to `0.4.8` on `master` produced a merge commit on `dev` that
  contains the bump.
- Still to verify on GitHub: a real bump PR merge in a test repository, a
  conflict case (job fails, `dev` unchanged), and a push that does not touch
  `version` (job skips).
