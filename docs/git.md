# Git workflow

Use one branch and one worktree for each task. Keep unrelated edits in their
original worktree. Read [Contributing](../CONTRIBUTING.md) before you start.

## 1. Branches

- `main` holds the production release.
- `beta` holds the next release. It includes the Rust backend.
- Start a task from `origin/beta`. Target `beta` when you open its pull request.
- Use a short task branch, such as `fix/catalog-search` or `docs/setup`.
- Keep the web, iOS, and Android clients in this repository. Each client has
  its own build and release.

## 2. Worktrees

Run these commands from your primary checkout:

```sh
git fetch origin --prune
git worktree add -b fix/catalog-search ../honorum-catalog-search origin/beta
cd ../honorum-catalog-search
make setup
npm --prefix apps/web exec -- playwright install chromium
```

The optional [T3 Code configuration](../t3.json) creates a worktree for each
new thread. It runs `make setup` and `make build`. Its Check and Demo actions
run `make check` and `make demo`.

Use `git worktree list` to find active worktrees. Remove a finished worktree
only after you commit or save its edits elsewhere.

```sh
git worktree remove ../honorum-catalog-search
```

## 3. Review and commit

1. Review the complete diff.
2. Run the checks for the changed code.
3. Run `make check` for structural or shared API changes.
4. Run `make test-ios` for iOS changes.
5. Stage the files that belong to the task.
6. Review the staged diff.
7. Commit the change.

```sh
git diff
make check
git add -p
git diff --cached
git commit -m "fix(catalog): preserve the selected term"
git push -u origin HEAD
gh pr create --draft --base beta
```

`git add -p` selects edits in tracked files. Add new files by name.
Describe the problem, the resulting behavior, and the checks in the pull request.

The [pre-push hook](development.md#5-pre-push-hook) runs local checks after
`make setup`. The repository has no hosted test workflow.

## 4. Update and merge

- Merge `origin/beta` into a shared task branch when it needs an update.
- Rebase only a branch you own. Coordinate before you rewrite a published branch.
- Use `--force-with-lease` when you must update a rebased task branch.
- Never force-push `main` or `beta`.
- Squash-merge a reviewed pull request after its checks pass.
- Delete the task branch after merge.
- Promote a tested beta release to `main` before production deployment.

## 5. Repository access

The upstream repository is `https://github.com/HonorumLLC/honorum`.
Use your own GitHub account and commit identity.

```sh
gh auth login
gh auth setup-git
git config --local user.name "Your Name"
git config --local user.email "YOUR_VERIFIED_OR_PRIVACY_EMAIL"
git config --local pull.ff only
git config --local fetch.prune true
git config --local push.default simple
```

Check your remotes with `git remote -v` before a push. External contributors
push to a fork. Keep credentials out of remote URLs, files, and commits.

## 6. Maintainer settings

- Require a pull request for `main` and `beta`.
- Block branch deletion and force pushes on both branches.
- Require resolved review conversations before merge.
- Allow squash merges and delete merged task branches automatically.
- Require checks only when an active workflow reports them.
- Keep the default Actions token read-only.
- Keep [CODEOWNERS](../.github/CODEOWNERS) current as maintainers change.
- Keep production credentials in the deployment environment.

These settings need changes on GitHub. This document does not apply them.

## 7. Releases

Use the [release procedures](deployment.md#4-release-procedures).
Backend and edge releases are independent.

Installed mobile clients can outlive an API deployment. Preserve supported
API contracts and check shared fixtures before a contract change.
Use a temporary release branch only when an older release needs separate fixes.
Never move or delete a published release tag.
