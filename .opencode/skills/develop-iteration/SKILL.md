---
name: develop-iteration
description: Use ONLY when the user explicitly requests /develop-iteration. Develops every committed issue in an iteration, parallelizes independent work, processes PR feedback, and autonomously merges ready PRs into non-main bases.
compatibility: opencode
metadata:
  opencode/autoinvoke: "false"
---


# develop-iteration — iteration delivery

Trigger: `/develop-iteration <iteration-number-or-milestone>` or `/develop-iteration` (uses the
active iteration).

## Purpose

Delivers the issues already assigned to an iteration. This skill does **not** choose the
iteration's scope: that decision belongs to `/scrum-master iteration-plan` and the user. It
coordinates one agent per issue, uses `/dev-issue` for implementation and `/process-pr` for
remediation, and merges every PR that satisfies the frozen rule.

The run is resumable. GitHub milestones, issues, branches, PRs, checks, reviews and comments are
the state: never rely on an in-memory queue as the only record of progress. On a restart,
reconstruct the plan from GitHub and continue without duplicating branches or PRs.

## Non-negotiable boundaries

- Never create, retarget or merge a PR whose base is `main`.
- Never push directly to `main` or `develop`.
- Merges may go into `develop` or into any other branch that is not `main`. Feature branches and
  their sub-branches are normal topology, not an exceptional cleanup.
- Every merge goes through `bash scripts/gh-automation.sh pr-merge <pr> --head <observed-sha>
  --base-head <observed-base-sha>`. The helper refuses `main` and a changed head or base, but it
  does not decide whether the PR is ready: the fresh check of Step 5 is mandatory.
- `NO_DEEPSEEK_REVIEW` remains human-only. Findings in the body of a DeepSeek review are
  acknowledged only through the `/process-pr` sequence (read, decide, public reply, fresh
  `DEEPSEEK_REVIEW_READ` label).
- Iteration ceremonies remain human-in-the-loop. Stop after delivery: do not automatically run
  either backlog refinement or iteration planning.
- The rules of `AGENTS.md` apply to every agent: isolated worktrees, no reading of `.env`, no new
  dependency without an explicit request, privacy checks before every push and every published
  body.

## 1. Resolve the iteration and reconstruct the state

With an argument, resolve it to **exactly one** open milestone. Accept the milestone number,
`Iteration N` or `Sprint N`. Without an argument use the same rule as `iteration-advance`: the
open milestone with the **lowest** sequence, read from the title and otherwise from the milestone
number. If two open milestones share the lowest sequence, stop and name both: choosing is a
product decision.

Read every non-ceremony issue (without the `ceremony` label) assigned to the milestone,
**including closed ones**, and find every PR that references it. For each issue record:

- state and the mandatory `/dev-issue` fields (read from the **body**, the way `/dev-issue` reads
  them);
- declared dependencies (`### Dependencies`) and ordering constraints written in the body;
- existing branches and PRs, with the PR's **actual base**;
- CI, DeepSeek, review-thread and mergeability state;
- the area (`backend`, `frontend`, `shared`) and the likely files from `### Code Pointers` and
  `### Technical Context`.

Work that has already landed is complete. Reuse open PRs and existing worktrees instead of opening
duplicates. A PR closed without merging, or a branch whose link to the issue is ambiguous, is a
stop condition for that issue, not permission to guess.

A dependency **outside** the iteration that is not yet on `develop` blocks the issue that depends
on it: report it, do not pull it forward.

## 2. Build the dependency and branch graph before assigning work

Build a directed acyclic graph from: explicit issue dependencies, the area ordering in
`AGENTS.md` (**backend first** with the `serde` types and the Tauri command, **then the frontend**
that uses it), PR bodies and base relationships between already open PRs. A PR into another
feature branch depends on that branch and must be merged before the parent branch's final PR can
go up.

Choose the base for every new issue:

- `develop` for an independent issue (the default);
- the relevant feature branch when the issue is a child of work deliberately assembled there;
- the actual base of every existing PR, without changing it;
- refuse `main` at planning time and check again right before creating the PR and before the
  merge.

Pass a non-default base explicitly to the issue's agent: `/dev-issue <issue> --base <branch>`.
The base is part of the plan: do not silently retarget an existing PR to simplify the graph.

When several child PRs need a new aggregate base, create it without opening the parent PR:

1. Choose a `feature/<descriptive-slug>` name (never `main`) and verify it does not exist yet.
2. In a temporary worktree `<repo-parent-directory>/redline-issue-iteration-<N>-aggregate`,
   create the branch from an up-to-date `origin/develop`, verify the worktree root, push the
   branch, then remove the worktree.
3. Pass that base to the child issues' agents.
4. When every planned child PR is merged, create the aggregate PR from the feature branch into
   `develop`. Check the full body with `bash scripts/check-body-privacy.sh` before publishing it,
   put `Closes #N` for every contained issue, then verify the base and body on GitHub.

Use an aggregate base only when the combined final PR plausibly stays under the review's 90,000
character cap and the feature reads well as a whole. Otherwise use PRs in dependency order
directly into `develop`.

Opening the parent PR only after assembly gives CI and the automatic DeepSeek round the combined
final diff. If a parent PR was already open when a child's merge changed its head, wait for fresh
CI and explicitly request a final DeepSeek review of the assembled head
(`bash scripts/gh-automation.sh pr-deepseek-force-review <pr>`) before considering it ready.

## 3. Decide the parallel wave

Parallelism is the default, but only for genuinely independent work. Two issues can be in the same
wave when **all** of these conditions hold:

- neither reaches the other in the dependency graph;
- neither produces a public contract (serialized types, Tauri command, state-file format) used by
  the other;
- their code pointers and the surfaces they will change do not substantially overlap (in this
  repository `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/store.rs` and
  `src/api.ts` are frequent touch points: two issues that both change them should normally be
  serialized);
- merging either one first cannot invalidate the other's acceptance criteria;
- the planned bases already exist and are not waiting on another wave.

When the evidence is insufficient, serialize and say why. Do not turn uncertainty into parallel
work. Recompute the graph after every merge: a boundary discovered during implementation or a PR
body can add an ordering constraint.

Use the **Task tool** to start exactly one owning agent for each issue in the ready wave, up to
the available concurrency slots. Start the whole maximal set together so it runs in parallel; the
remaining safe issues wait for the next wave. Give each agent the issue number, the planned base,
the relevant dependencies and this instruction:

> Run `/dev-issue <issue> --base <planned-base>` and follow it to the end. Report every PR
> created (including split or stacked ones), with the base, branch and URL of each, and every
> newly discovered dependency. Do not merge. You remain the owning agent for follow-up
> `/process-pr` work on those PRs.

Never two agents on the same issue. If an issue must be split into several PRs, its agent remains
their owner while the orchestrator adds those PRs and their base edges to the graph. When the
owning agent has already returned its report, follow-up work goes to that same agent, with its
context intact: in Claude Code with `SendMessage`, in another runtime with the equivalent
mechanism for resuming a sub-agent. Only if the runtime does not allow resuming it, start a new
agent for the same issue, after the previous one has finished, and pass it the previous one's
report.

## 4. Watch and remediate the set of open PRs

When an implementation wave returns, watch all of its open PRs as one set. Before waiting on a
PR, verify that it is still open. Pending CI or a first DeepSeek review still in progress is a
wait, not a reason to call `/process-pr`. The DeepSeek review can take tens of minutes (see
"Timing reality" in `/process-pr`).

**Watch the set with a single persistent watch built on `pr-status-json`.**
Start it as soon as the run's first PR exists: in Claude Code with the `Monitor` tool and
`persistent: true`, which runs the loop in the background and delivers every stdout line as a
notification while the loop continues; in another runtime with a background process whose output
you read. **Never run the loop as an ordinary foreground shell command**: it never returns, it is
cut off at the timeout and the lines do not arrive as they are emitted. The loop's `sleep 60` runs
only inside that background process; the orchestrator does not wait with a foreground `sleep`,
and between events control returns to the user. The `last` state lives for the whole lifetime of
the process, so each PR is emitted only when it changes. Act on every line: `ready:true` goes to Step 5; threads, unread findings,
failing CI or `CONFLICTING` go to the owning agent's `/process-pr`; only a pending state
waits.

Set `ISSUES` to the iteration's issue numbers, which every `/dev-issue` branch carries in its
name, and `EXTRA_PRS` to the numbers of PRs whose branch does not carry an issue number (the
aggregate `feature/<slug>` PR; empty if there are none):

```bash
cd <repo-root>
ISSUES='<issue>|<issue>|<issue>'
EXTRA_PRS='<pr>|<pr>'
if [ "${BASH_VERSINFO[0]:-0}" -lt 4 ]; then
  echo "watch: bash 4 or newer required (associative arrays); found ${BASH_VERSION:-unknown}"; exit 1
fi
declare -A last
while true; do
  if ! all=$(gh pr list --state open --limit 100 --json number,headRefName,headRefOid \
      --jq '.[] | "\(.number) \(.headRefOid[:7]) \(.headRefName)"'); then
    echo "watch: gh pr list failed"; sleep 60; continue
  fi
  prs=$(printf '%s\n' "$all" | grep -E "/(${ISSUES})-|^(${EXTRA_PRS:-none}) " || true)
  while read -r n sha _; do
    [ -n "$n" ] || continue
    s=$(bash scripts/gh-automation.sh pr-status-json "$n" 2>/dev/null | jq -ce '
      ["ready_to_merge","ci_pending","ci_failing","required_check_state","unresolved_threads",
       "deepseek_unread_findings","deepseek_review_complete","deepseek_rounds_exhausted",
       "mergeable"] as $need
      | if ($need - keys | length) == 0 then {ready:.ready_to_merge,
      pending:.ci_pending, failing:.ci_failing, gate:.required_check_state,
      threads:.unresolved_threads, unread:.deepseek_unread_findings,
      ds_done:(.deepseek_review_complete or .deepseek_rounds_exhausted), mergeable:.mergeable}
        else empty end' 2>/dev/null) || s=""
    [ -n "$s" ] || s="status-unreadable"
    if [ "${last[$n]:-}" != "$sha $s" ]; then last[$n]="$sha $s"; echo "PR #$n @$sha $s"; fi
  done <<<"$prs"
  sleep 60
done
```

It emits every matching PR once on the first pass and again when its head or state changes; a
state that cannot be read is emitted as `status-unreadable`, never as silence. The same holds for
a renamed or missing `pr-status-json` field: `jq -e` with no result fails, and the line is emitted
as `status-unreadable` instead of a mute `null`
(`scripts/test/agent-skills-sync.test.sh` binds the names to the helper). Every
read that can fail is guarded (`|| continue`, `|| true`, `|| s=""`, `${last[$n]:-}`), so the loop
keeps this promise even under `set -euo pipefail`: an unguarded failed assignment would end the
loop without a line, which is exactly the silence this watch exists to prevent. For the same
reason the loop checks the bash version first: `declare -A` requires bash 4, and the stock
`/bin/bash` on macOS is 3.2, where it would fail without a line; so it exits immediately with a
message (on macOS: `brew install bash`, then run the loop with that bash). The branch filter
anchors on `/<issue>-` and says nothing about the prefix (`feature/`, `fix/`, `refactor/`).
**Watch the first pass before trusting it:** a watch that has printed nothing for a PR that
exists is broken, not quiet; until it has printed, read `pr-status-json` by hand after every
wave.

When an aggregate PR is created after the start, stop the watch (in Claude Code with `TaskStop`,
elsewhere by stopping the background process) and restart it with its number in `EXTRA_PRS`. Stop
it once delivery is finished: a persistent loop does not end on its own.

**Never build the watch on `gh pr checks --json`.** The repository's minimum `gh` (2.18.0,
`GH_MIN_VERSION` in `scripts/gh-automation.sh`) has no `--json` on `pr checks`: the call exits
with `unknown flag: --json`, and a loop that discards stderr reads the empty output as "no checks
yet" and skips the PR on every pass. In voxelheim-v2 (Iteration 60) exactly such a loop stayed
silent for hours while two PRs were `ready_to_merge`. `pr-status-json` reads the same rollup as
the frozen rule, so the watch and the merge decision cannot diverge.

When a PR has failing CI to fix, unresolved DeepSeek threads, unread body findings
or `mergeable == CONFLICTING`, send its owning agent a task: run
`/process-pr <pr>`. A missing `ci-gate` together with `CONFLICTING` is conflict remediation, not
a wait for CI: no `pull_request` workflow can start until the merge ref exists.
Keep one agent per issue even when the issue has several PRs. Do not run forced cycles in
parallel on several PRs of the same issue, and avoid unnecessary concurrent DeepSeek polling,
because of GraphQL rate limits.

Persist the remediation count in public comments on the PR, so a restart does not reset the
safety cap. Before every forced cycle count the comments carrying the marker
`<!-- develop-iteration:remediation:pr-N:attempt-K -->`, refuse attempt four, check the new
comment with `bash scripts/check-body-privacy.sh` and post it before dispatching the agent.
Re-read the state after every push. Stop that PR early when `/process-pr` reports an
infrastructure failure, an unclear finding without an evidence-based disposition, an unsafe
rebase or another condition that requires the user's judgement. Never force an extra DeepSeek
round just to make the loop move: the cap is **one** automatic round per PR (`MAX_ROUNDS`).

## 5. Merge ready leaves in dependency order

Only leaf PRs whose dependencies have already been merged are candidates. The commands below use
`REPO=$(gh repo view --json nameWithOwner --jq .nameWithOwner)`: define it first, even when
resuming from this step. Immediately before every merge:

1. Re-read the PR's state, `headRefOid`, base branch and **full body**. Read the base SHA from
   the PR's REST shape (`gh api "repos/$REPO/pulls/<pr>" --jq '.base.sha'`), because the minimum
   `gh` does not expose `baseRefOid` in `gh pr view --json`. Keep the two SHAs as
   `OBSERVED_HEAD` and `OBSERVED_BASE_HEAD`.
2. Verify that it is still open and that the base is neither empty nor `main`.
3. Verify that the observed head contains the observed base, on the same two SHAs that item 6
   passes to `pr-merge`, without depending on the local clone's refs:
   `gh api "repos/$REPO/compare/$OBSERVED_BASE_HEAD...$OBSERVED_HEAD" --jq '.status'`. Only
   `ahead` or `identical` proceed; `behind`, `diverged`, an error or empty output mean the PR is
   not a candidate: see "After a merge" below.
4. Honour every ordering constraint written in its issue and in the PR body.
5. Run `bash scripts/gh-automation.sh is-ready-to-merge <pr>` on the current head.
6. If it succeeds, run
   `bash scripts/gh-automation.sh pr-merge <pr> --head "$OBSERVED_HEAD" --base-head "$OBSERVED_BASE_HEAD"`.

`READY TO MERGE` is useful evidence but can lag behind the current state: never substitute a
stale label for item 5. Conversely, a successful fresh evaluation of the frozen rule is enough
even if the event-driven labeler has not applied the label yet.

Feature branches are not covered by the `develop` and `main` rulesets. For them the fresh
frozen-rule check is the only protection on the checks the orchestrator controls: never skip it.

### After a merge

**The branch that moved invalidates the evidence of the PRs that sit on it**, even if their head
did not change. After a merge, refresh every open PR whose **head or base** is the branch that
just moved, `develop` included. Every PR based on the moved branch goes through its owning agent
with `/process-pr`: Step 2b merges the new base into the head without rebasing or force-pushing,
runs the gates of the affected areas and pushes; then fresh CI is required before the PR becomes
a candidate again. Re-read the DeepSeek state: if the round is already spent, do not wait for an
automatic review that will never start, because the base commits were reviewed on their own PRs.
A head that was green before says nothing about a base that just changed. For a parent PR whose
head is the feature branch, the assembled-head rule of Step 2 applies, including the explicit
forced review.

**There is no post-merge verification on `develop`.** In this repository CI runs only on
`pull_request` and there is no integration workflow on `develop`: the only evidence that two PRs
work together is the second one's CI, re-run after incorporating the first. That is why the
reconciliation above also applies to `develop` and is mandatory: never merge into `develop` a PR
whose head does not contain the current tip of `develop`, and merge into `develop` **one PR at a
time**, waiting for the next one's fresh CI.

A child PR merged only into a feature branch does not reliably close its issue, and its
`Closes #N` does not carry over to the parent PR. Keep those issues pending until the feature
branch reaches `develop`, and make sure the parent PR's body carries `Closes #N` for every issue
whose commits it contains. For a parent PR that is already open, write the full corrected body to
a temporary file, check it with `bash scripts/check-body-privacy.sh`, use
`bash scripts/gh-automation.sh pr-edit <pr> --body-file <path>` (never `gh pr edit`, which fails
on `gh` 2.45), then re-read the full body and every ordering constraint. Close an issue by hand
only after verifying that `develop` contains its commits.

## 6. Continue until the iteration is delivered

After every merge, rebuild the issue/PR graph and start the next maximal safe wave. Continue until
every non-ceremony issue in the milestone is closed by a merged PR and no planned child or parent
PR remains open.

Do not close the milestone and do not run the ceremonies created by `iteration-lifecycle.yml`:
they are product decisions outside delivery. Closing the last issue starts the backlog refinement
ceremony on its own.

## Stop conditions

When you can, stop only the affected line of work; stop the whole run when continuing could break
the ordering or the shared base. Report the exact state and the command to resume when any of
these happens:

- mandatory fields missing from an issue body, or ambiguous iteration or dependency data;
- a `main` base requested or already present on a PR;
- a dependency outside the iteration that is not yet on `develop`;
- an infrastructure failure or unreadable GitHub state;
- a change that exceeds the review cap and cannot be split;
- three failed remediation cycles on the same PR;
- a PR that `/process-pr` cannot reconcile with its base without a judgement about intent;
- a decision that changes the iteration's scope or requires a new dependency.

## Final report

Report the milestone, which agent owns which issue, the branch and PR topology, the merges in
order, the CI and DeepSeek outcomes, the state of `develop` after the last merge (the last green
CI of the most recently merged PR) and any stopped work. If every issue has landed, say that
delivery is complete and that the next transition belongs to the completion-driven iteration
lifecycle.

## Reference

- Issue implementation: `/dev-issue`
- PR remediation: `/process-pr`
- Frozen state and guarded merge: `scripts/gh-automation.sh`
- Iteration selection and lifecycle: `AGENTS.md`
- CI: `.github/workflows/ci.yml`
