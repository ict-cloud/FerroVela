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
   - Otherwise it runs `git merge --no-edit <master sha>` on `dev` and pushes
     `dev` with `GITHUB_TOKEN`.

Result: `dev` contains every commit from `master`. When `dev` has no commits of
its own, the merge is a fast-forward and the two branches point at the same
commit.

## Behaviour and limits

- **Trigger scope**: only pushes to `master`. Pushes to `dev` do not start it,
  so the workflow cannot loop.
- **Concurrency**: runs are serialized (`cancel-in-progress: false`) so a
  merge is never cancelled half-way.
- **Conflicts**: if the merge conflicts (for example, `dev` also edited the
  `version` line or `Cargo.lock`), the job fails and nothing is pushed. Resolve
  the merge on `dev` manually.
- **Branch protection**: `GITHUB_TOKEN` must be allowed to push to `dev`. If
  `dev` is protected, grant the Actions bot bypass rights or the push will fail.
- **Pushes made with `GITHUB_TOKEN` do not trigger other workflows**, so the
  sync push to `dev` does not start the test workflow there.
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
