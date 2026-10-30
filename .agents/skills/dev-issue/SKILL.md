---
name: dev-issue
description: Implements GitHub issues end-to-end. Use to turn an issue into a merge-ready PR.
---


# dev-issue — from issue to PR

Trigger: `$dev-issue <issue-number>` or `$dev-issue <issue-url>`, with an optional
`--base <branch>` when the work has to be stacked on a feature branch other than `develop`.

## Purpose

Takes a GitHub issue from requirements to PR, statelessly. It exits after opening the PR and
cleaning up the worktree. It does **not** watch the PR: passive monitoring belongs to
`pr-labeler.yml` (event-driven plus a sweep every six hours), active remediation belongs to
`$process-pr`.

It uses `git worktree` to isolate the branch, so several issues can progress in parallel without
touching the main working directory.

## Workflow

### Step 1: read the issue

`gh issue view <number> --json title,body,labels`. These fields are read from the body:

| Field | Section in the body | Required | Used for |
|-------|---------------------|----------|----------|
| Area | `### Area` → dropdown | Yes | Working directory and gates |
| Type | `### Type` → dropdown | Yes* | Branch prefix (*bugs have no Type: it is derived from the `bug` label) |
| Priority | `### Priority` → dropdown | Yes* | How much effort (*same exception) |
| User Story | `### User Story` | Yes* | Goal (*same exception) |
| Acceptance Criteria | `### Acceptance Criteria` | Yes* | Verification checklist (*same exception) |
| Technical Context | `### Technical Context` | Yes | Implementation guide |
| Out of Scope | `### Out of Scope` | Yes | Implementation boundaries |
| Code Pointers | `### Code Pointers` | No | Files to read first |
| Dependencies | `### Dependencies` | No | Blocking work |
| Test Strategy | `### Test Strategy` | No | What to test |

The required fields mirror `.github/ISSUE_TEMPLATE/feature_request.yml`. Bugs
(`bug_report.yml`) have `What happened? / What should have happened? / Steps to reproduce`
instead of Type/Priority/User Story/Criteria: the steps to reproduce are the acceptance criteria.

If a required field is missing, do not guess: say which one is missing and ask the user whether
to proceed.

### Step 2: validate the area

| Value in the issue | Directory | Gate (Step 6) |
|--------------------|-----------|---------------|
| `backend (Rust / Tauri)` | `src-tauri/` | backend |
| `frontend (React / TypeScript)` | `src/` (plus `index.html`, `vite.config.ts`, `tsconfig*.json`, `public/`) | frontend |
| `shared (Cross-cutting)` | root | backend and frontend |

**Defensive validation**: if the value is not one of these, do NOT proceed. Ask: "I don't
recognize the area '[value]'. Allowed values: backend (Rust / Tauri), frontend (React /
TypeScript), shared (Cross-cutting). Which one should I use?"

### Step 3: read the context

Before writing code:

1. Read `AGENTS.md` at the root: conventions and the pipeline contract.
2. Read the documents in `docs/` that the issue touches: `architecture.md` for the backend
   modules, `decisions.md` for choices already made, `roadmap.md` for the milestone.
3. Open every **Code Pointer** in the issue.
4. Read **Out of Scope** and keep it in mind: touch nothing on that list.
5. Look for existing patterns with `rg --files` and `rg`.

### Step 3a: choose the PR base

`BASE_BRANCH=develop`, unless the invocation passes `--base <branch>`. The base decides both the
branch starting point and the PR target.

Verify that the remote branch exists before creating the worktree. Reject an empty base and
reject `main`: this skill never opens or retargets PRs to `main`. If you reuse an existing branch
or PR, verify that the actual base is the requested one instead of silently changing it.

### Step 4: create the worktree and branch

**Branch name:**

```
Type (from the dropdown, or from the labels if missing):
  feature     → feature/
  enhancement → feature/
  refactor    → refactor/
  bug label   → fix/

Slug: title → lowercase → non-alphanumeric characters to hyphens
      → consecutive hyphens collapsed → no leading/trailing hyphens
      → at most 5 words

Result: <prefix><issue-number>-<slug>

Examples:
  feature/12-custom-fan-curve
  fix/31-replay-missing-microphone-track
  refactor/40-extract-lactd-client
```

The worktree path is derived from the repo root, never from `$PWD`: a relative
`../redline-issue-N` after a `cd src-tauri` would create the worktree *inside* the repo.

```bash
set -euo pipefail

REPO_ROOT=$(git rev-parse --show-toplevel)
cd "$REPO_ROOT"

SLUG=$(echo "<issue-title>" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9]/-/g' | sed 's/-\+/-/g' | sed 's/^-//;s/-$//' | tr '-' '\n' | head -5 | paste -sd '-')
BRANCH="<prefix><issue-number>-${SLUG}"
WORKTREE_DIR="$(dirname "$REPO_ROOT")/redline-issue-<issue-number>"
BASE_BRANCH="<develop-or-explicit-base>"
[ -n "$BASE_BRANCH" ] && [ "$BASE_BRANCH" != "main" ] || { echo "Refusing PR base: $BASE_BRANCH"; exit 1; }
git fetch origin "$BASE_BRANCH"

# Reuse a worktree already open for this branch.
EXISTING=$(git worktree list --porcelain | awk -v b="refs/heads/$BRANCH" '
  /^worktree /{wt=substr($0,10)}
  /^branch /{if (substr($0,8)==b) {print wt; exit}}')

if [ -n "$EXISTING" ]; then
  echo "Reusing existing worktree: $EXISTING"
  WORKTREE_DIR="$EXISTING"
  cd "$WORKTREE_DIR"
else
  # Three states, not two: the branch can exist without a worktree (Step 10 removes the
  # worktree and keeps the branch, which holds the PR's commits). `worktree add -b`
  # would fail with "A branch named '<branch>' already exists".
  if git show-ref --verify --quiet "refs/heads/$BRANCH"; then
    echo "Branch $BRANCH exists with no worktree — checking it out"
    git worktree add "$WORKTREE_DIR" "$BRANCH"
  else
    git worktree add -b "$BRANCH" "$WORKTREE_DIR" "origin/$BASE_BRANCH"
  fi
  cd "$WORKTREE_DIR"
fi

# Dependencies: node_modules is not shared between worktrees.
pnpm install --frozen-lockfile
(cd src-tauri && cargo fetch --locked)
```

The worktree is `<repo-parent-directory>/redline-issue-<number>`:

- main repo: `<workspace>/redline`
- worktree for issue #12: `<workspace>/redline-issue-12`

**Verify before modifying anything:**

```bash
[ "$(git rev-parse --show-toplevel)" = "$WORKTREE_DIR" ] || { echo "NOT in worktree — abort"; exit 1; }
```

From here on every change and every git command runs inside `$WORKTREE_DIR`. If the check
fails, stop and go back into the worktree: do not modify the main directory.

The worktree has no `.env` file and must not have one: the gates use no secrets, and agents
never read `.env`.

### Step 5: decide how many PRs, then implement

**Before writing code, decide how many PRs the issue must be split into.** Step 7 measures the
finished diff against `DEEPSEEK_MAX_DIFF_CHARS` (**90,000** characters): past that limit the
reviewer truncates the diff and every unread file becomes a finding that blocks the PR. Splitting
after writing everything is the most expensive moment, because every boundary has to be carved
out of finished work.

Estimate from the issue, not by feel:

- count the **areas** touched (backend, frontend);
- count the **surface kinds** the issue names: a mechanism, a data model, a Tauri command, a UI
  screen or control, persistent state, a client for an external source. One kind is one PR; two
  kinds are normally two PRs;
- count the **acceptance criteria**: past six or seven, the groups they fall into are already
  the boundaries of the parts;
- remember that tests weigh: a module with real tests is worth about three times the production
  code.

Aim for about 80,000 characters per part, not 90,000: the limit is a truncation point, not a
target. If the issue body already states a number of PRs, it is a minimum to verify, not an
answer: redo the estimate and, if it does not add up, say so in the PR description and follow
yours.

**The boundary is one the code already draws**: the Tauri command and the UI that uses it, the
mechanism and its caller, the data model and its persistence. Never a character count. Every
part must compile, pass the gates and stand on its own. In a binary crate "on its own" has a
meaning for the compiler: a `pub` item reachable only from tests is `dead_code` under
`-D warnings`. Move the boundary or use `#[allow(dead_code)]` with a comment naming the part
that will use it, and say so in the PR.

Later parts are opened on the previous part's branch (`--base <previous-branch>`): CI runs on PRs
to any base. Every description states which part it is, how many there are, the exact base and
the direction in which the stack must be closed (the deepest ready leaf merges first into its
base, then the base moves up).

**Write the estimate in the PR description**, next to the Step 7 measurement.

Then implement:

1. **Respect Out of Scope to the letter**: if a change touches something on that list, stop and
   reassess.
2. Follow existing patterns.
3. **System access stays in the backend.** sysfs, the `lactd` socket, the GPU Screen Recorder
   processes and the disk are touched only from Rust; the frontend displays data and calls Tauri
   commands.
4. **Redline never runs as root** and never writes to sysfs: every GPU change goes through
   `lactd`, with confirmation or rollback to the previous values (`confirm_pending_config`).
5. No new dependencies (crates or npm packages) without explicit instruction.
6. No secrets in the code, no debug `dbg!`/`println!`/`console.log` left in production paths.
7. Surgical changes: modify existing files when you can, create new ones only when needed.
8. GPU Screen Recorder (GPL-3.0) is used only as an external process: never copy its code.
9. Every change in `src/` follows the "Interface: inspired by Adrenalin, ruled by simplicity"
   section of `AGENTS.md`; the PR says in one line how it complies.

### Step 6: quality gates (MANDATORY)

These commands mirror `.github/workflows/ci.yml`. Whatever you skip here becomes a red PR and a
wasted DeepSeek review round: the budget is one round.

Inside the worktree, for **every** area touched by the diff
(`git diff --name-only "origin/$BASE_BRANCH"...HEAD` plus the changes not yet committed):

| Area | Command |
| ---- | ------- |
| backend (`src-tauri/**`) | `cd src-tauri && cargo fmt --all --check && cargo clippy --all-targets --locked -- -D warnings && cargo build --locked && cargo test --locked` |
| frontend (`src/**`, `public/**`, `index.html`, `vite.config.ts`, `tsconfig*.json`) | `pnpm install --frozen-lockfile && pnpm build && pnpm test` |
| `package.json` / `pnpm-lock.yaml` | both rows above |

`pnpm build` is `tsc && vite build`: type-check and bundle together. `pnpm test` runs the
`src/**/*.test.ts` files with `node --test` (Node 22.18 or later strips the types by itself).

**A gate's verdict is its exit status, and a pipeline's is that of its last command.**
`cargo clippy ... 2>&1 | tail -5 && echo OK` prints `OK` on a failed clippy: clippy exits 1,
`tail` exits 0. The chains above use `&&` for this reason: run them as written. If the output is
too long, keep the status and trim the text separately:

```bash
status=0
cargo clippy --all-targets --locked -- -D warnings > /tmp/clippy.txt 2>&1 || status=$?
tail -20 /tmp/clippy.txt
[ "$status" -eq 0 ]   # ← the verdict, read from the gate and not from the pager
```

`|| status=$?` and not `; status=$?`: with `set -e` active (Step 4 enables it) a failed command
outside an `||` list closes the shell before `tail` shows why.

**Every PR also runs the automation suite**, including documentation-only ones:

```bash
failed=0
for t in scripts/test/*.test.sh; do
  printf '== %s\n' "$t"
  bash "$t" || { echo "FAILED: $t"; failed=1; }
done
python3 .github/scripts/test_deepseek_review.py || failed=1
[ "$failed" -eq 0 ]  # ← the block's exit status IS the verdict; do not remove this line
```

The glob is the instruction: do not list the files by hand, the list would fall behind the
directory. The last line is load-bearing: with `break` instead of the flag the loop would exit 0
on a failed test.

If you modify a skill in `.claude/skills/`, regenerate the adapters
(`bash scripts/sync-agent-skills.sh`) and commit the three copies together.

Formatting is the most frequently skipped gate and the one that turns CI red most often. Fix it
with `cargo fmt --all`, follow clippy's messages, then rerun the whole gate.

**Definition of Done (verified before the PR):**

- [ ] Formatting clean (`cargo fmt --check`)
- [ ] Lint clean (`cargo clippy -D warnings`)
- [ ] Backend (`cargo build --locked`) and frontend (`pnpm build`) builds succeed
- [ ] Tests green (`cargo test --locked`)
- [ ] Automation suite green
- [ ] No debug prints in production code
- [ ] No secrets, tokens or keys in code or tests
- [ ] API key only in the backend, never in the frontend or in logs
- [ ] Publication privacy clean: `check-publication-privacy.sh` on the index and on every
      commit of the branch, `check-commit-privacy.sh` on the author and messages of the
      branch's commits

### Step 7: commit, then measure what the reviewer will have to read

DeepSeek writes its reasoning in the same output budget as the verdict: a diff that is too large
can exhaust it and return nothing. The `DEEPSEEK_MAX_DIFF_CHARS` limit is **90,000** characters;
beyond it the diff is truncated and every unread file becomes a finding that blocks the PR until
someone reads it and declares it publicly. A human can give this acknowledgement; `$process-pr`
can do it only with its documented, dated, evidence-based sequence. Neither is an outcome to aim
for.

**Commit first, then measure.** `git diff "origin/$BASE_BRANCH"...HEAD` compares commits: on an
uncommitted tree it answers `0` whatever the pending work.

```bash
git add -A
git commit -m "<conventional-commit-type>: <concise description>

Implements #<issue-number>

- <main change>
- <main change>"

# What the reviewer actually sees: generated code and lockfiles are excluded by
# name, so exclude them here too. `wc -m` and not `wc -c`: the limit is in characters.
# Two pathspecs per lockfile: without `:(glob)` a pathspec is anchored at the root, so
# `Cargo.lock` alone does not exclude `src-tauri/Cargo.lock`. The reviewer matches the
# basename at any depth (`is_generated_path` in .github/scripts/deepseek_review.py);
# scripts/test/diff-measure-parity.test.sh keeps the two lists aligned.
REVIEWABLE=$(git diff "origin/$BASE_BRANCH"...HEAD -- . \
  ':(exclude)gen/*' ':(exclude)*/gen/*' ':(exclude)*_generated.*' \
  ':(exclude)Cargo.lock' ':(exclude)*/Cargo.lock' \
  ':(exclude)go.sum' ':(exclude)*/go.sum' \
  ':(exclude)pnpm-lock.yaml' ':(exclude)*/pnpm-lock.yaml' | wc -m)
echo "reviewable diff: ${REVIEWABLE} characters (cap 90,000)"
```

**Measure against the base the part will actually be opened on.** A part born from the previous
part and opened on it measures only itself. Measuring it against `develop` while the previous
part is not merged also counts that part's commits.

**If you need to change a PR's base, do it before the push.** Changing the base fires
`pull_request` with the `edited` action, which is not among the default activity types: on its
own it reruns nothing. Change the base first, then push, so it is the push that starts CI.

**If the measurement exceeds the limit, the estimate was wrong and the work must be split before
opening any PR**, along a boundary like those in Step 5, into as many parts as needed. If the
parts cannot stand on their own, say so and ask the user instead of opening pieces that do not
compile.

Record in the PR the Step 5 estimate next to the measurement: two numbers make the estimate
verifiable.

### Step 8: push and PR

Before the push, the two privacy checks: the content (the index and every commit the branch
adds, one by one) and the commit metadata (author, committer and message). Before creating the PR, the body. A failed check
blocks publication even with all gates green: remove the data before continuing, without
reprinting it in another message. A PR body describes what you just did, and you did it in a
directory with a name: write `<worktree>` or `<repo-root>`, never the path.

```bash
# From the worktree, with the Step 7 commit already made:
bash scripts/check-publication-privacy.sh "origin/$BASE_BRANCH" HEAD
bash scripts/check-commit-privacy.sh "origin/$BASE_BRANCH" HEAD
git push -u origin HEAD

PR_BODY=$(mktemp)
trap 'rm -f "$PR_BODY"' EXIT
cat >"$PR_BODY" <<'EOF'
## Summary

Closes #<issue-number>

### Changes
- <main changes>

### Review size
- Estimate before implementation: <estimate and reasoning>
- Measured diff: <REVIEWABLE> characters

### Gates run
- [x] cargo fmt / clippy
- [x] backend and frontend build
- [x] tests — all green
- [x] automation suite

### Acceptance criteria verification
<every criterion, checked or unchecked with the explanation>

### Out of Scope (verified)
<confirmation that nothing on the Out of Scope list was touched>
EOF

bash scripts/check-body-privacy.sh <"$PR_BODY"
gh pr create \
  --base "$BASE_BRANCH" \
  --title "<issue-title>" \
  --body-file "$PR_BODY"
```

**To fix a PR's title or body, do not use `gh pr edit`.** On `gh` 2.45.0 its Projects classic
pre-fetch fails before applying any flag. Use
`bash scripts/gh-automation.sh pr-edit <pr> --title "<title>" --body-file <path>`, which writes
via REST and rereads the fields before declaring success.

Check only the boxes you actually ran. An empty box with one line of reasoning is useful; a
checked and false one poisons every later review.

**PR target**: `develop` by default; an explicit `--base` can point to another feature branch.
Never open or retarget a PR to `main`, never push directly to `main`, never merge into `main`.
Merging into bases other than `main` is authorized, but **not by this skill**: `$dev-issue`
exits at PR creation, before CI has said anything.

### Step 9: exit

Report to the user:

- branch name
- PR number and URL
- worktree directory
- that the DeepSeek review will start on its own with `deepseek-pr-review.yml`

Do **NOT** wait for CI, do not check reviews, do not merge.

### Step 10: remove the worktree

```bash
# Go back to the main repo (derived, not hard-coded)
MAIN_REPO=$(git rev-parse --path-format=absolute --git-common-dir)
MAIN_REPO=$(dirname "$MAIN_REPO")
cd "$MAIN_REPO"

git worktree remove "$WORKTREE_DIR"
git worktree prune
```

Confirm: "Worktree removed: `$WORKTREE_DIR`".

If `git worktree remove` fails (uncommitted changes), warn the user and give the command for
manual cleanup. `node_modules` and `src-tauri/target` are ignored by git and do not block the
removal.

## References

- Issue templates: `.github/ISSUE_TEMPLATE/`
- Pipeline: `AGENTS.md`
- Gates (source of truth): `.github/workflows/ci.yml`
- Branch protection: `.github/branch-protection.md`
- Helper: `scripts/gh-automation.sh`
