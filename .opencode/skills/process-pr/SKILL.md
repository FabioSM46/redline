---
name: process-pr
description: Use ONLY when the user explicitly requests /process-pr. Manages PR feedback loops. Use to resolve CI failures and review feedback on open PRs.
compatibility: opencode
metadata:
  opencode/autoinvoke: "false"
---


# process-pr — PR remediation

Trigger: `/process-pr <pr-number>` or `/process-pr` (uses the current branch's PR).

## Purpose

A remediation cycle for conflicts, failing CI and review feedback. It can be started by the user or
by an orchestrator.

The passive path (`pr-labeler.yml`) covers the normal case. This skill is for when you want a
result now.

**All work happens in a `git worktree`**, never in the main directory. If a worktree already
exists for the PR's branch (from a `/dev-issue` or `/process-pr` run that was not cleaned up), it is
reused.

## Timing reality — read before touching the polling

The DeepSeek review job has a budget of `DEEPSEEK_REQUEST_TIMEOUT_SECONDS` (2700) ×
(`DEEPSEEK_MAX_RETRIES` (1) + 1) = 90 minutes, under a `timeout-minutes: 100` with 10 minutes
reserved for setup and posting. Real run times depend on the diff size; the diff cap is 90,000
characters, and a PR close to the cap can use a good part of the budget.
**A wait shorter than the budget does not "poll": it times out and reports stale state.**
`DEEPSEEK_WAIT_SECONDS` below is 5700 (95 minutes); it covers the budget and ends before the job
cap. Do not lower it without changing the workflow first.

The round budget is `MAX_ROUNDS: "1"` (in `.github/workflows/deepseek-pr-review.yml`). Exactly one
full review (Mode A) is automatic; thread replies (Mode B) do not spend the budget and continue
without limit. So the normal execution of this skill is **a single pass**.

`pull_request` runs use the merge ref, so the workflow that runs is the **base branch's** copy.
To diagnose review behaviour, read the version of `deepseek-pr-review.yml` on the PR's actual
base.

## Workflow

### Step 0: Common setup

```bash
set -euo pipefail
REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
REPO_ROOT=$(git rev-parse --show-toplevel)
DEEPSEEK_WAIT_SECONDS=5700
POLL_INTERVAL=30
```

### Step 1: PR number

If the user provided one, use it. Otherwise find the PR for the current branch:

```bash
gh pr list --head "$(git branch --show-current)" --json number --jq '.[0].number'
```

If there is no PR, exit with an error.

### Step 2: Create or reuse the worktree and detect the areas

```bash
PR_META=$(gh pr view <pr-number> --json headRefName,baseRefName,state)
BRANCH=$(echo "$PR_META" | jq -er '.headRefName | select(length > 0)')
BASE_BRANCH=$(echo "$PR_META" | jq -er '.baseRefName | select(length > 0)')
[ "$(echo "$PR_META" | jq -r '.state')" = "OPEN" ] || exit 1
git fetch origin "$BRANCH" "$BASE_BRANCH"
REMOTE_HEAD=$(git rev-parse "origin/$BRANCH")

# Reuse a worktree already checked out for this branch.
# substr() rather than $2: worktree paths can contain spaces.
WORKTREE_DIR=$(git worktree list --porcelain | awk -v b="refs/heads/$BRANCH" '
  /^worktree /{wt=substr($0,10)}
  /^branch /{if (substr($0,8)==b) {print wt; exit}}')

if [ -n "$WORKTREE_DIR" ]; then
  echo "Reusing existing worktree: $WORKTREE_DIR"
  cd "$WORKTREE_DIR"
  [ -z "$(git status --porcelain)" ] || {
    echo "Existing worktree is dirty; preserving it and stopping"
    exit 1
  }
  LOCAL_HEAD=$(git rev-parse HEAD)
  if [ "$LOCAL_HEAD" = "$REMOTE_HEAD" ]; then
    :
  elif git merge-base --is-ancestor "$LOCAL_HEAD" "$REMOTE_HEAD"; then
    git merge --ff-only "origin/$BRANCH"
  else
    echo "Existing worktree has local commits or diverged history; preserving it and stopping"
    exit 1
  fi
else
  WORKTREE_DIR="$(dirname "$REPO_ROOT")/redline-pr-<pr-number>"
  if git show-ref --verify --quiet "refs/heads/$BRANCH"; then
    [ "$(git rev-parse "refs/heads/$BRANCH")" = "$REMOTE_HEAD" ] || {
      echo "Local branch differs from origin; preserving it and stopping"
      exit 1
    }
    git worktree add "$WORKTREE_DIR" "$BRANCH"
  else
    git worktree add -b "$BRANCH" "$WORKTREE_DIR" "origin/$BRANCH"
  fi
  cd "$WORKTREE_DIR"
fi

pnpm install --frozen-lockfile
(cd src-tauri && cargo fetch --locked)

[ "$(git rev-parse --show-toplevel)" = "$WORKTREE_DIR" ] || { echo "NOT in worktree — abort"; exit 1; }
```

**Detect every touched area, not just the first.** The classification is the one in
`scripts/changed-areas.sh`, which is also the one CI uses:

```bash
AREAS=$(gh pr view <pr-number> --json files --jq '.files[].path' | bash scripts/changed-areas.sh)
echo "$AREAS"   # backend=true|false, frontend=true|false
```

**Every PR runs the automation suite**, including documentation-only ones. The automation tests
read files across the whole repo, so no path selector exempts them. If a PR changes a skill, also
regenerate the adapters.

**Gates — they mirror `.github/workflows/ci.yml`. Run one per detected area:**

Run each chain as it is written and take its exit status as the verdict. Piping it into `head` or
`tail` returns the pager's success instead of the gate's; `/dev-issue` Step 6 explains why.

| Area | Command (from `$WORKTREE_DIR`) |
| ---- | ------------------------------ |
| `backend` | `cd src-tauri && cargo fmt --all --check && cargo clippy --all-targets --locked -- -D warnings && cargo build --locked && cargo test --locked` |
| `frontend` | `pnpm install --frozen-lockfile && pnpm build && pnpm test` |
| Every PR | the full `scripts/test/*.test.sh` suite (glob it, never retype the list) plus `python3 .github/scripts/test_deepseek_review.py`, under a failure flag so a red test is the block's exit status, as in `/dev-issue` Step 6 |

Formatting is the gate most often skipped and the one that most often reddens CI.

#### Step 2b: Reconcile the current base

Do this on every remediation run. If the current base tip is not an ancestor of the PR head, merge
the base into the head without rebasing or force-pushing, resolve the conflicts, run every gate of
the affected areas and push. This also invalidates stale CI on a technically mergeable child PR
after its feature base advances.

```bash
BASE_HEAD=$(git rev-parse "origin/$BASE_BRANCH")
if ! git merge-base --is-ancestor "$BASE_HEAD" HEAD; then
  [ "$(git rev-parse "origin/$BRANCH")" = "$REMOTE_HEAD" ] || exit 1
  git merge --no-ff --no-commit "origin/$BASE_BRANCH"
  # Resolve conflicts; if their intent is ambiguous, abort and stop without publishing.
  # Run every affected gate, then:
  git add -A
  git commit -m "fix: reconcile ${BASE_BRANCH} on PR #<pr-number>"
  bash scripts/check-publication-privacy.sh "origin/$BASE_BRANCH" HEAD
  bash scripts/check-commit-privacy.sh "origin/$BASE_BRANCH" HEAD
  git push origin HEAD
  REMOTE_HEAD=$(git rev-parse HEAD)
fi
```

After this push, fresh CI is required. The base commits were already reviewed on their own PRs, so
do not wait for an automatic review that cannot start once the budget is spent.

### Step 3: PR status

```bash
bash scripts/gh-automation.sh pr-status <pr-number>
```

This evaluates the full frozen READY TO MERGE rule and prints a `[FAIL]` line for each unmet
condition. There are **seven** conditions, all required:

1. Unresolved (non-outdated) review threads = 0
2. No review requesting changes
3. No failing CI checks
4. No pending CI checks
5. No *missing* required check: the stable `ci-gate` check must have run and succeeded
6. `mergeable == MERGEABLE`
7. DeepSeek has finished (clean verdict, rounds exhausted or `NO_DEEPSEEK_REVIEW` exemption), with
   no unread findings in a review body (cleared with the `DEEPSEEK_REVIEW_READ` label)

Conditions 5 and 6 exist because **a conflicting PR runs no checks at all**: with nothing red, a
naive read would call it green. A conflict is actionable remediation, not a wait; Step 2b merges
the current base into the PR head so CI can run again.

The helper fails closed: an unreadable value counts as failure, never as success.

### Step 4: Process the DeepSeek review round (in the worktree)

With `MAX_ROUNDS: "1"` this is a **single pass**. Repeat 4a–4e only if `max_rounds` is raised in
the workflow or after an explicit `pr-deepseek-force-review`. Safety cap: **5 iterations**.

#### 4a — Read the DeepSeek state

```bash
ROUNDS=$(bash scripts/gh-automation.sh pr-deepseek-rounds <pr-number>) || true
if ! echo "$ROUNDS" | jq -e '.' >/dev/null 2>&1; then
  echo "ERROR: pr-deepseek-rounds returned invalid JSON. Raw output: ${ROUNDS:0:200}"
  exit 1
fi
# The helper reports configuration/API failures as {"error":...} with a non-zero exit,
# so a zero count always means "no reviews", never "lookup failed".
if echo "$ROUNDS" | jq -e '.error' >/dev/null 2>&1; then
  echo "ERROR: pr-deepseek-rounds failed: $(echo "$ROUNDS" | jq -r '.error')"
  exit 1
fi
REVIEW_COMPLETE=$(echo "$ROUNDS" | jq -r '.review_complete')
ROUNDS_EXHAUSTED=$(echo "$ROUNDS" | jq -r '.review_rounds_exhausted')
BOT_REVIEW_COUNT=$(echo "$ROUNDS" | jq -r '.bot_review_count')
PREV_REVIEW_ID=$(echo "$ROUNDS" | jq -r '.latest_review_id')
ROUND_CAP=$(echo "$ROUNDS" | jq -r '.max_rounds')
echo "DeepSeek state: rounds=$BOT_REVIEW_COUNT/$ROUND_CAP complete=$REVIEW_COMPLETE exhausted=$ROUNDS_EXHAUSTED"
```

**Guard**: if `review_complete=true` or `review_rounds_exhausted=true`, there is no round to wait
for. Skip 4b and go to 4c: a round that already landed still has threads to address.

#### 4b — Wait for the review (only if none has landed)

Only when `PREV_REVIEW_ID` is `0`.

```bash
START_TIME=$(date +%s)
while true; do
  sleep "$POLL_INTERVAL"

  # The deadline check goes FIRST, before any guard that can `continue`:
  # otherwise a persistently failing helper would loop forever.
  ELAPSED=$(($(date +%s) - START_TIME))
  if [ "$ELAPSED" -ge "$DEEPSEEK_WAIT_SECONDS" ]; then
    echo "Timeout (${DEEPSEEK_WAIT_SECONDS}s) waiting for DeepSeek — the job may be approaching its own 100-minute cap."
    echo "Check: gh run list --workflow=deepseek-pr-review.yml --limit 3"
    break
  fi

  CURRENT=$(bash scripts/gh-automation.sh pr-deepseek-rounds <pr-number>) || true
  if ! echo "$CURRENT" | jq -e '.' >/dev/null 2>&1; then
    echo "WARNING: pr-deepseek-rounds returned invalid JSON, retrying... (${CURRENT:0:200})"
    continue
  fi
  if echo "$CURRENT" | jq -e '.error' >/dev/null 2>&1; then
    echo "WARNING: pr-deepseek-rounds failed ($(echo "$CURRENT" | jq -r '.error')), retrying..."
    continue
  fi
  NEW_ID=$(echo "$CURRENT" | jq -r '.latest_review_id')

  # A new review ID first: even a review with review_complete=true carries feedback.
  if [ "$NEW_ID" != "0" ] && [ "$NEW_ID" != "$PREV_REVIEW_ID" ]; then
    echo "DeepSeek review arrived (id=$NEW_ID)"
    PREV_REVIEW_ID="$NEW_ID"
    BOT_REVIEW_COUNT=$(echo "$CURRENT" | jq -r '.bot_review_count')
    break
  fi

  if [ "$(echo "$CURRENT" | jq -r '.review_complete')" = "true" ]; then
    echo "DeepSeek approved with no issues found"
    break
  fi

  echo "Waiting for DeepSeek review... (elapsed: ${ELAPSED}s / ${DEEPSEEK_WAIT_SECONDS}s)"
done
```

A timeout here is a real signal. A job cancelled by the 100-minute cap produces **no output at
all**, and `pr-status-json` counts `CANCELLED` as failing: the PR stays at `needs-work` with
nothing in the log explaining why. If that happened, say so explicitly.

#### 4c — Address the round's feedback

1. **Fetch the unresolved, non-outdated threads**:

   ```bash
   bash scripts/gh-automation.sh pr-comments <pr-number>

   gh api graphql --raw-field 'query=query($owner:String!,$repo:String!,$pr:Int!){repository(owner:$owner,name:$repo){pullRequest(number:$pr){reviewThreads(first:50){nodes{id isResolved isOutdated comments(first:2){nodes{databaseId body}}}}}}}' \
     -f owner="${REPO%%/*}" -f repo="${REPO##*/}" -F pr=<pr-number> \
     --jq '.data.repository.pullRequest.reviewThreads.nodes[] | select(.isResolved == false and .isOutdated == false) | {id, comments: [.comments.nodes[] | {databaseId, body}]}'
   ```

2. **Fix every thread** (files under `$WORKTREE_DIR/`):
   - valid bug or suggestion → implement it
   - question → answer it
   - nitpick under 5 minutes → just fix it
   - out of scope → reply explaining why; do not silently ignore it

   **Publication order:** prepare the dispositions in items 3–5, but do not send replies, resolve
   threads, post the body audit or write `DEEPSEEK_REVIEW_READ` until Step 4e has pushed every
   fix and verified the remote head. A fix that exists only in the worktree has not landed.

3. **Prepare a reply for every thread**, including the ones you did not act on. Record the comment
   ID, thread ID and text for Step 4f; no GitHub writes yet.

4. **Prepare to resolve** only the threads whose fix will have landed or whose point is genuinely
   addressed. Record the decision for Step 4f.

   A thread whose suggestion you **rejected** may be resolved, but only after a reply that states
   the evidence: that reply is the whole audit trail. Never resolve a thread you have not
   answered.

5. **Findings in the review body** create no thread and cannot be resolved. Fetch the complete
   bodies of the recent DeepSeek reviews, not just the threads, and read every finding before
   deciding whether to acknowledge them:

   ```bash
   ACK_STATE=$(bash scripts/gh-automation.sh pr-deepseek-rounds <pr-number>)
   ACK_REVIEW_ID=$(echo "$ACK_STATE" | jq -er '.latest_review_id | select(. != 0)')
   if ! BODY_REVIEWS=$(gh api graphql \
     --raw-field 'query=query($owner:String!,$repo:String!,$pr:Int!){repository(owner:$owner,name:$repo){pullRequest(number:$pr){reviews(last:100,states:[APPROVED,CHANGES_REQUESTED,COMMENTED]){nodes{databaseId submittedAt state author{login} body}}}}}' \
     -f owner="${REPO%%/*}" -f repo="${REPO##*/}" -F pr=<pr-number>); then
     echo "Could not fetch DeepSeek review bodies; acknowledgement is blocked"
     exit 1
   fi
   echo "$BODY_REVIEWS" | jq --arg bot "${DEEPSEEK_BOT_USER:-github-actions[bot]}" '
     def canon: sub("\\[bot\\]$"; "");
     .data.repository.pullRequest.reviews.nodes[]
     | select((.author.login // "" | canon) == ($bot | canon))
     | select((.body // "") != "")
     | {databaseId, submittedAt, state, body}'
   ```

   Address every valid point in code. A rejected point needs concrete evidence from the diff, a
   test or a measured behaviour: disagreement without evidence is not a disposition. If a finding
   is unclear or has no evidence either way, leave it unacknowledged and report the block.

   Record one disposition per body finding for Step 4f. Never use `NO_DEEPSEEK_REVIEW`: that
   exemption remains human-only.

#### 4d — Fix CI

```bash
bash scripts/gh-automation.sh pr-status <pr-number>
```

If CI is failing, read the logs (`gh run view <run-id> --log-failed`), fix it, then run the Step 2
gate for **every** affected area before pushing.

If the problem is infrastructure (runner, package mirror, expired token), report it and stop. Do
not paper over it with code changes.

#### 4e — Commit and push

If the feedback requires correcting the PR's title or body, **never use `gh pr edit`**: on `gh`
2.45.0 it fails before applying any flag. Use
`bash scripts/gh-automation.sh pr-edit <pr> --title "<title>" --body-file <path>`.

```bash
cd "$WORKTREE_DIR"
git add -A
if git diff --cached --quiet; then
  echo "No source changes to commit"
else
  git commit -m "fix: address feedback on PR #<pr-number>"
  bash scripts/check-publication-privacy.sh "origin/$BASE_BRANCH" HEAD
  bash scripts/check-commit-privacy.sh "origin/$BASE_BRANCH" HEAD
  git fetch origin "$BRANCH"
  [ "$(git rev-parse "origin/$BRANCH")" = "$REMOTE_HEAD" ] || {
    echo "PR head moved; preserve local commits and reconstruct state"
    exit 1
  }
  git push origin HEAD
  REMOTE_HEAD=$(git rev-parse HEAD)
fi

# Confirm the PR is still open at REMOTE_HEAD. Only then execute the prepared
# replies/resolutions and body acknowledgement sequence from steps 3–5.
CURRENT_PR=$(gh pr view <pr-number> --json state,headRefOid)
[ "$(echo "$CURRENT_PR" | jq -r '.state')" = "OPEN" ] || exit 1
[ "$(echo "$CURRENT_PR" | jq -r '.headRefOid')" = "$REMOTE_HEAD" ] || exit 1
```

**NEVER run `git add` / `git commit` / `git push` from the main directory.** Always from
`$WORKTREE_DIR`.

With the round budget spent, this push does **not** trigger another review. If another pass is
genuinely needed:

```bash
bash scripts/gh-automation.sh pr-deepseek-force-review <pr-number> [ref]
```

`ref` defaults to `develop` and must be a branch that carries the bypass, because the dispatched
run executes that ref's workflow definition.

#### 4f — Publish dispositions after the push

Only after Step 4e has verified the remote head, publish every prepared reply, then resolve its
thread:

```bash
gh api "repos/$REPO/pulls/<pr-number>/comments/<comment-databaseId>/replies" \
  --field body="<response naming the pushed fix or the rejection evidence>" || exit 1
gh api graphql \
  -f query='mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{id isResolved}}}' \
  -f id="<THREAD_ID>" || exit 1
```

For body findings, post one disposition per finding (fixed, with file/test, or rejected, with the
evidence), after passing the exact text to `scripts/check-body-privacy.sh`.
This is the public audit trail. Then re-read `latest_review_id`: it must
still equal `ACK_REVIEW_ID`, otherwise repeat this step from the body fetch.
Refresh `DEEPSEEK_REVIEW_READ` only after the audit trail exists, treating the label lookup as
three-state (present/absent/unreadable), and verify both the fresh label and the postconditions:

```bash
ACK_COMMENT='<one bullet per DeepSeek body finding, with review ID and evidence>'
printf '%s\n' "$ACK_COMMENT" | bash scripts/check-body-privacy.sh || exit 1
gh pr comment <pr-number> --body "$ACK_COMMENT" || exit 1
LATEST_BEFORE_ACK=$(bash scripts/gh-automation.sh pr-deepseek-rounds <pr-number> \
  | jq -er '.latest_review_id')
[ "$LATEST_BEFORE_ACK" = "$ACK_REVIEW_ID" ] || exit 1
if bash scripts/gh-automation.sh pr-check-label <pr-number> DEEPSEEK_REVIEW_READ; then
  bash scripts/gh-automation.sh pr-label <pr-number> remove DEEPSEEK_REVIEW_READ || exit $?
  if bash scripts/gh-automation.sh pr-check-label <pr-number> DEEPSEEK_REVIEW_READ; then
    echo "Stale acknowledgement label is still present"
    exit 1
  else
    LABEL_RC=$?
    [ "$LABEL_RC" -eq 1 ] || exit "$LABEL_RC"
  fi
else
  LABEL_RC=$?
  [ "$LABEL_RC" -eq 1 ] || exit "$LABEL_RC"
fi
bash scripts/gh-automation.sh pr-label <pr-number> add DEEPSEEK_REVIEW_READ || exit $?
if ! bash scripts/gh-automation.sh pr-check-label <pr-number> DEEPSEEK_REVIEW_READ; then
  echo "Could not verify the fresh acknowledgement label"
  exit 1
fi
LATEST_AFTER_ACK=$(bash scripts/gh-automation.sh pr-deepseek-rounds <pr-number> \
  | jq -er '.latest_review_id')
UNREAD_AFTER_ACK=$(bash scripts/gh-automation.sh pr-status-json <pr-number> \
  | jq -er '.deepseek_unread_findings')
if [ "$LATEST_AFTER_ACK" != "$ACK_REVIEW_ID" ] || [ "$UNREAD_AFTER_ACK" != "0" ]; then
  bash scripts/gh-automation.sh pr-label <pr-number> remove DEEPSEEK_REVIEW_READ
  exit 1
fi
```

A newer review stays unread because its timestamp follows the label. If a disposition is not
understood or has no evidence, leave the label absent. Never apply `NO_DEEPSEEK_REVIEW`.

### Step 5: Final verification

```bash
bash scripts/gh-automation.sh pr-status <pr-number>
bash scripts/gh-automation.sh pr-check-label <pr-number> "READY TO MERGE"
```

`pr-labeler.yml` applies the label when CI completes, on the six-hour sweep and on manual
dispatch. If `pr-status` says the rule is satisfied but the label is missing, the labeler has not
run yet.

### Step 6: Remove the worktree

```bash
MAIN_REPO=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")
cd "$MAIN_REPO"

git worktree remove "$WORKTREE_DIR"
git worktree prune
```

If `git worktree remove` fails, warn the user and provide the manual cleanup command.

### Step 7: Report

```
PR #<number>: <title>
├── Mergeable:   ✅ no conflicts
├── CI:          ✅ ci-gate ran and passed
├── DeepSeek:    ✅ clean verdict / rounds exhausted
├── Threads:     ✅ 0 unresolved
├── Body:        ✅ no unread findings (or: N unacknowledged, with the reason)
├── Label:       READY TO MERGE
└── Status:      Ready for an autonomous merge into a non-main base
```

Report each line from what `pr-status` actually printed.

**Do not merge from this skill.** This is a separation of responsibilities: the merge decision for
non-main bases belongs to whoever reads the finished result and the PR's ordering constraints.

## Guardrails

- **Worktree only**: edits, git and gates inside the worktree, never in the main directory.
- **Verify the worktree**: before any edit, `git rev-parse --show-toplevel` must be
  `$WORKTREE_DIR`.
- A single pass by default (`MAX_ROUNDS: "1"`); a cap of 5 iterations if the budget is raised.
- Run the gates of **every** touched area.
- If CI fails because of infrastructure, report it and stop.
- **Never poll `mergeable`, and never block on it.** A **merged** PR reports it as `null`
  forever, so waiting for it to change is not slow, it is impossible. `pr-status` retries it
  internally and the labeler re-reads it on every pass. Report what you measured and exit.
- **Before waiting on anything about a PR, check that it is still open.**
- If DeepSeek does not arrive within `DEEPSEEK_WAIT_SECONDS` (5700), report it as a probable job
  timeout and exit the loop: do not treat it as "no findings".
- **GraphQL limits**: one wait is at most 190 requests (5700s ÷ 30s) at about 3–5 points each,
  under 950 points. GitHub allows 5000 per hour. Avoid parallel forced cycles across several PRs.
- NEVER push directly to `main`, and never merge a PR into `main`: both are human-only. Merging
  into non-main bases is authorized through
  **`bash scripts/gh-automation.sh pr-merge <pr> --head <observed-sha> --base-head
  <observed-base-sha>`**: it refuses a `main` base, fails closed on an unreadable base and
  rejects a head or base that moved after the read. Read the PR body before merging: an ordering
  stated against another PR binds whoever merges, and the frozen rule cannot see it.
- Never force-push or rebase without an explicit user instruction. Conflicts are resolved by
  merging the base into the PR head, preserving history.
- Always run the gates locally before publishing fixes, and the two privacy checks
  (`check-publication-privacy.sh`, `check-commit-privacy.sh`) before every push.
- Apply `DEEPSEEK_REVIEW_READ` only through the read/dispose/public-audit/fresh-write sequence of
  steps 4c and 4f. Never `NO_DEEPSEEK_REVIEW`.

## Reference

- Helpers: `scripts/gh-automation.sh` (`pr-status`, `pr-deepseek-rounds`, `pr-deepseek-force-review`)
- DeepSeek reviewer: `.github/workflows/deepseek-pr-review.yml` / `.github/scripts/deepseek_review.py`
- Passive monitor: `.github/workflows/pr-labeler.yml`
- Gates (source of truth): `.github/workflows/ci.yml`
- Branch protection: `.github/branch-protection.md`
