# Redline — Tauri 2 (Rust + React)

Control panel for **AMD GPUs on Linux**, modelled on AMD Adrenalin but leaner: real-time
statistics, in-game overlay, tuning, instant replays and recording. Native desktop app: **Rust**
backend with **Tauri 2**, **React + TypeScript** frontend (Vite, pnpm). Context, decisions,
architecture and roadmap are in `docs/`.

## Repository Structure

```
redline/
├── AGENTS.md             # This file — global rules (authoritative for the pipeline)
├── CLAUDE.md             # Points Claude Code to this file
├── README.md             # Project overview
├── src/                  # React + TypeScript frontend
├── src-tauri/            # Rust workspace: Tauri app and configuration
│   └── crates/
│       ├── redline-core/ # Sensors (sysfs), lactd client, GPU Screen Recorder control
│       └── redline-probe/# CLI to try out pieces of redline-core by hand
├── docs/                 # context, decisions, architecture, roadmap
├── scripts/              # gh-automation.sh, CI classifier and gate, automation tests
├── .github/              # CI, DeepSeek review, PR labeler, iteration lifecycle
├── .claude/skills/       # Canonical Claude skills (/dev-issue, /process-pr, /develop-iteration, /scrum-master)
├── .agents/skills/       # Generated Codex adapters ($dev-issue, $process-pr, $develop-iteration, $scrum-master)
└── .opencode/skills/     # Generated OpenCode adapters (/dev-issue, /process-pr, /develop-iteration, /scrum-master)
```

## Areas

| Area       | Folders | Technology | Responsibility |
| ---------- | ------- | ---------- | -------------- |
| `backend`  | `src-tauri/` | Rust (edition 2024), Tauri 2 | Tauri commands, reading sensors from sysfs, `lactd` client, launching and controlling GPU Screen Recorder, configuration and local state |
| `frontend` | `src/`, `public/`, `index.html`, `vite.config.ts`, `tsconfig*.json` | React 19, TypeScript, Vite | Dashboard, charts, tuning and recording panels, `invoke()` calls |
| `shared`   | root | — | Changes that touch both areas or the shared configuration (`package.json`, `pnpm-lock.yaml`) |

`redline-core` does not depend on Tauri: all the logic that talks to the system lives there, and
the Tauri app in `src-tauri/src/` exposes it as commands. `redline-probe` is for trying out a
piece without the UI.

### System access stays in the backend — always

The frontend shows data and calls Tauri commands. sysfs, the `lactd` socket, the GPU Screen
Recorder processes, signals and the disk are touched only from Rust. A command that hands the
frontend a path to open or a command to run is a bug even when it "works".

### GPU Rules

- **Redline never runs as root and never writes to sysfs.** Reads from sysfs are free; every
  change (fans, power limit, clocks, voltages, profiles) goes through the `lactd` daemon.
- **Every tuning change is confirmed.** `lactd` applies the change as pending: the app must ask
  the user for confirmation and send `confirm` or `revert` (`confirm_pending_config`).
  An interface that confirms on its own, without the user seeing the result, is a bug.
- **Undervolting and overclocking** require the `amdgpu.ppfeaturemask` kernel parameter: the app
  explains it, it does not set it.
- **GPU Screen Recorder is GPL-3.0**: it is used only as an external process (options, signals,
  stdout, IPC). Never copy or adapt its code. LACT is MIT.
- **A missing tool is a disabled feature, not an error.** Without `lactd` tuning is disabled and
  the app says what to install; without GPU Screen Recorder the same goes for replays; a sensor
  the GPU does not expose is shown as absent (`—`), not as zero.
- **Tool versions are pinned**: the `lactd` API and the GPU Screen Recorder options are not
  stable promises. A version change is verified against their source.

### Cross-Cutting Changes

1. Backend first: logic in `redline-core`, serialized types (`serde`) and the Tauri command.
2. Then the frontend, against the command's real behaviour.
3. Run the gates of both areas before opening the PR.

### Platform: Linux Only

Redline exists for Linux: it reads the kernel's `amdgpu` driver and uses Linux-only tools. There
is no Windows or macOS version and CI does not build one. In practice:

- **Paths only with `Path`/`PathBuf`** and `join`; config and data folders are asked of Tauri
  (`app.path()`), never `~` written in the code.
- **No particular distribution**: no `apt` or Ubuntu paths in the code. The reference target is
  Ubuntu LTS, but Fedora and Arch must work.
- **Wayland first**, X11 must work. Under Wayland screen capture goes through the portal
  (`-w portal`) and global shortcuts through the GlobalShortcuts portal, which is not available
  on every desktop (GNOME has it since 48): an alternative is always needed.
- **Tests do not depend on the machine's GPU**: `redline-core` tests read fake sysfs trees built
  in a temporary folder, never the real `/sys`. CI has no AMD GPU.
- **Multiple GPUs**: a machine can have an integrated and a discrete GPU. No code that takes
  `card0` or a single card for granted.

### Interface: inspired by Adrenalin, ruled by simplicity

The UI follows Adrenalin's organisation (dashboard, performance, recording, settings) but keeps
only what is actually used. The layout is fixed by D8 in `docs/decisions.md`: four tabs (Home,
Performance, Record, Settings), an always-visible status bar, and a tray icon that keeps the app
running when the window is closed. In practice:

- **Few surfaces.** A screen does one thing; advanced options sit behind an "Advanced", not all
  in view.
- **Numbers up front.** Temperatures, clocks, power and FPS are readable at a glance: tabular
  figures, units always visible, clean charts without decoration.
- **Colour sparingly.** Neutral palette with a single accent (Redline red); colour signals state
  (high temperature, active recording, error), it does not decorate. Light and dark are both
  mandatory: colours are CSS variables in `:root` of `src/App.css`, with the dark value as
  `--dark-*`, never written in individual components.
- **Tokens, not values.** Type sizes, spacing, radii and colours are the variables at the top of
  `src/App.css`.
- **States always handled**: loading, missing tool (with a sentence that says what to do) and
  error, without blocking browser dialogs.
- **No third-party branding**: no AMD, Radeon or Adrenalin names, logos or colours in the UI or
  in the package.
- **No component or style library** without an explicit decision: plain CSS as long as it is
  enough.

A PR that touches `src/` describes in one line how it respects these rules.

## Public Repository Privacy Boundary

The repository is **public**: everything that gets published is readable forever: files, commit
metadata (author and committer), branch names, commit and PR messages, review comments, CI logs,
artifacts. That is why these rules apply.

- **Never a real email address.** Commits use the `noreply` address provided by GitHub
  (copy it from Settings → Emails, do not type the number by hand). Code, fixtures and messages
  may contain only GitHub `noreply` identities and addresses on reserved example domains.
- **Never paths from your own machine.** No usernames, home directories, mount points or
  checkout locations. Documentation uses placeholders such as `<repo-root>`, `<worktree>` and
  `<workspace>`; tests use clearly synthetic paths.
- **Never personal data** in issues, PRs or code: real names, phone numbers, locations, account
  identifiers. The project's public identity is the `@FabioSM46` handle.
- **Never secrets or operational identifiers.** API keys, tokens, credentials, internal
  hostnames and non-public endpoints live in GitHub Secrets or in ignored local files (`.env`),
  never in git, logs, issues or PRs.
- **Never a reference to an agent session.** A `Claude-Session:` trailer or a
  `claude.ai/code/session_…` link in a message is an endpoint tied to an account.
- **Never attribute a commit to an account that is not yours.** An
  `<id>+<login>@users.noreply.github.com` address credits whoever owns `<id>`, not `<login>`: the
  wrong number next to the right name credits a stranger. The three privacy scripts keep a
  login → id table for the identities the repository publishes.

**Three scripts split the work, and share the same three categories** (non-public email, machine
path, private network address), copied into each and kept identical by
`scripts/test/commit-privacy.test.sh`:

- `bash scripts/check-publication-privacy.sh` reads the content of the files in the index (what
  a push publishes, not the working folder); with `<base> <head>` it also reads the tree of every
  commit in the range, because data added in one commit and removed in the next leaves the tip
  clean but stays in the published history; with `--all-history` it reads every reachable commit;
- `bash scripts/check-commit-privacy.sh <base> <head>` reads the author, committer and message of
  every commit the branch adds;
- `bash scripts/check-body-privacy.sh` reads an issue or PR body from stdin. Use it before
  publishing: `bash scripts/check-body-privacy.sh < body.md`.

Run the first two before every push, both with the branch range
(`"origin/$BASE_BRANCH" HEAD`): CI runs them in the `detect` job on the PR range, and a violation
blocks the PR even with everything else green. Diagnostics name only the location and the
category, never the value found; when a check fails, remove the data before continuing instead of
printing it again in another message.

**An issue or PR body can only be checked afterwards.** `.github/workflows/body-privacy.yml`
runs on `issues` and `pull_request_target` (`opened`, `edited`): if it finds something it adds the
`needs-privacy-review` label and a comment with the category and line; it does not close, edit or
hide anything. **It cannot prevent a leak, it can only shorten the time nobody knows about it**:
a green run means "nothing is published now", never "nothing was ever published". It is the
surface a machine writes most often: an agent writing a PR body transcribes what it has just
done, and it did it in a folder with a name.

**Before the first push and periodically**: run `check-publication-privacy.sh --all-history`
and `check-commit-privacy.sh` over the whole history. The scripts recognise only the three
categories (emails, paths, private network addresses) and `.env` files: they are not a secret
scanner. API keys and tokens of other shapes are invisible to them, so also run a dedicated
scanner (for example gitleaks) over the whole history. Then reread by hand the titles and bodies
of issues and PRs already open and check the secrets.

## Shared Conventions

- **No secrets in code**: keys, tokens and credentials live in GitHub Secrets or in ignored local
  files, never in git, logs, issues or PRs.
- **English only**: code, comments, user-facing strings, documentation, skills, issue
  templates, issues, PRs and commit messages are written in English (D7 in `docs/decisions.md`).
- **Commits**: never commit without an explicit request; conventional commit style (`feat:`,
  `fix:`, `refactor:`, `docs:`, `ci:`).
- **Read first, then write**: look for existing patterns before implementing.
- **No new dependencies** (crates or npm packages) without an explicit instruction.
- **Never read `.env` files**: AI agents MUST NOT read, open or inspect any `.env` file. Redline
  uses no secrets today; if it ever needs them, the list of variables will be in `.env.example`.
- **No tool signatures**: no agent `Co-Authored-By` trailers, no "Generated with ..." in PRs, no
  `claude.ai/code/session_…` links in messages.
- **NEVER push or merge to `main`**: `main` is the release branch and stays human-only. The AI
  must never run `git push origin main` or merge a PR into `main`. PRs targeting `main` are
  allowed; the human does the merge.
- **Merging into bases other than `main` is up to the AI**, at its own judgement. `READY TO MERGE`
  is a signal to read, not a precondition. The merge is done with
  **`bash scripts/gh-automation.sh pr-merge <pr>`**, which refuses a `main` base by name and
  fails closed on an unreadable base. See "What actually stops a merge" below.
- **Isolation with git worktrees**: every change and every git operation happens in an isolated
  `git worktree`, never in the main folder. Use `<parent-folder>/redline-issue-<number>` when the
  work is tied to an issue; if there is no issue do not invent a number, use
  `<parent-folder>/redline-issue-<descriptive-slug>`. `/process-pr` worktrees are
  `redline-pr-<number>`. Check that you are in the worktree before editing
  (`git rev-parse --show-toplevel`). After the PR is opened remove it with
  `git worktree remove` and `git worktree prune`. Reuse an existing worktree for the same branch.

## Branch Strategy

- `main` — release branch
- `develop` — integration branch and default base (the repository's default branch); stacked
  sub-branches may start from their feature branch
- `feature/<issue>-<slug>` — features and improvements
- `fix/<issue>-<slug>` — fixes
- `refactor/<issue>-<slug>` — restructuring without behaviour changes

For authorised work without an issue, omit the number instead of inventing it:
`<type>/<descriptive-slug>` (for example `refactor/extract-lactd-client`).

## AI-Driven Development Pipeline

The repository has a pipeline that takes issues from creation to merge-ready PRs, on three
layers:

| Layer        | Technology                              | Responsibility |
| ------------ | --------------------------------------- | -------------- |
| **Skills**   | Skills for Claude, Codex and OpenCode   | Implementation, PR handling, iteration delivery, scrum ceremonies |
| **Workflows**| GitHub Actions (`.github/workflows/`)   | CI, PR review, labelling, iteration transitions |
| **Helper**   | `scripts/gh-automation.sh`              | Shared wrapper for the REST and GraphQL APIs |

The whole infrastructure is ported from wow-don, which in turn took it from voxelheim-v2; the
comments in the workflow and script code cite issue and PR numbers from those repositories, where
the full history lives.

### Cross-Runtime Skill Synchronization

`.claude/skills/` is the canonical source. Codex uses the generated adapters in `.agents/skills/`,
OpenCode the ones in `.opencode/skills/`. When a canonical skill changes:

1. Edit only `.claude/skills/<skill>/SKILL.md`.
2. Run `bash scripts/sync-agent-skills.sh` from the root.
3. Run `bash scripts/test/agent-skills-sync.test.sh` and commit the three versions together.

Never edit the generated adapters by hand. CI fails if an adapter is stale.

### Pipeline Flow

```
Issue opened from the template
        │
        ▼
/dev-issue <number> in Claude/OpenCode, or $dev-issue <number> in Codex
        │
        ▼
skill: reads the issue → validates the area → reads AGENTS.md and docs/
        │
        ▼
skill: worktree + branch from the base (develop by default) → implements → gates (fmt, clippy, build, test)
        │
        ▼
skill: opens the PR against that base → EXITS (stateless)
        │
        ▼
deepseek-pr-review.yml: inline review comments on the diff ────────┐
        │                                                           │
        ▼                                                           ▼ (in parallel)
pr-labeler.yml (when CI/review finish + a sweep every 6h): reads ci-gate and threads
        │
    ┌───┴──────────────┐
    │ Frozen rule?      │──yes─→ label READY TO MERGE
    │ CI red?           │──yes─→ label needs-work
    │ otherwise         │──────→ needs-review
    └───────────────────┘
        │
        ▼
/process-pr (Claude/OpenCode) or $process-pr (Codex): conflicts, CI, feedback
        │
        ▼
The AI reads the result and the ordering constraints → merge into bases other than main
```

`/develop-iteration` (Claude/OpenCode) or `$develop-iteration` (Codex) runs all of this for the
issues already assigned to a milestone: one agent per issue with `/dev-issue`, parallel waves only
for independent work, `/process-pr` on the PRs that need fixing, and merging of the ready leaves
in dependency order. On `develop` there is no post-merge verification (CI runs only on
`pull_request`): that is why the skill merges one PR at a time into `develop` and realigns every
other PR to the new tip before merging it.

### Skills Reference

| Skill          | Claude / OpenCode         | Codex                     | Behaviour |
| -------------- | ------------------------- | ------------------------- | --------- |
| `dev-issue`    | `/dev-issue <number>`     | `$dev-issue <number>`     | Stateless: issue → PR → exits |
| `process-pr`   | `/process-pr <pr-number>` | `$process-pr <pr-number>` | Active remediation: conflicts, CI, review feedback |
| `develop-iteration` | `/develop-iteration [iteration]` | `$develop-iteration [iteration]` | Parallel agents per issue → fixes → merge of PRs into bases other than main |
| `scrum-master` | `/scrum-master <ceremony>` | `$scrum-master <ceremony>` | backlog-refine, iteration-plan, feature-spec |

Skills can be started by an agent as well as by a person: do not use
`disable-model-invocation`. Keep `allowed-tools` tight: they are the reason to prefer a skill over
a hand-built agent.

Iteration ceremonies keep the human in the loop: choosing which issues go into the next iteration
is the user's decision. `/develop-iteration` delivers the chosen scope and stops: it does not
close the milestone and does not run the ceremonies.

#### What actually stops a merge

**Into `develop`: the rulesets and the AI's judgement.** The branch ruleset requires a PR,
resolved review threads and (once the second ruleset is enabled) a green `ci-gate`. The frozen
rule computes `READY TO MERGE`; it is a signal, not a block.

**Into another feature branch: a fresh check by whoever merges.** The rulesets cover only
`develop` and `main`. Before the merge run `is-ready-to-merge` immediately before `pr-merge`,
and pass `--head <observed-sha>` and `--base-head <observed-base-sha>` so that a moved head or
base invalidates the reading.

**Into `main`: the instruction, plus a check on the designated path.** A deny can block
`git push origin main` because the command names the ref. **`gh pr merge <n>` names no
branch**: the base is a property of the PR, so no `Bash(...)` prefix tells a merge into
`develop` from one into `main`. In its place there is `pr-merge`, which reads `baseRefName` and
refuses `main`, failing closed on an empty or unreadable base without attempting the merge
(`scripts/test/pr-merge-guard.test.sh`).

**It stops an accident, not a decision.** `gh pr merge` and
`gh api -X PUT repos/OWNER/REPO/pulls/N/merge` bypass the helper, and nothing stops an agent that
wants to use them. The allowlist is not a sandbox: the skills have `Bash(gh *)` and `Bash(bash *)`,
and both are necessary. What holds `main` is the instruction.

The `deny` entries in `.claude/settings.json` for `git push origin main` (and the `--force`,
`-f`, `+main` variants) remain as a convenience: underneath there is a complete remote-side block.

**Residual risk.** Nothing reads a PR before it reaches `develop` unless the AI does.
`unresolved_threads == 0` does not mean a person has looked: the AI can resolve a thread it has
replied to, even when rejecting the suggestion, and that reply is the entire audit trail. Ordering
between PRs is invisible to the frozen rule: an order written in a PR body binds whoever merges.
**Read the bodies.**

### Frozen Acceptance Rule

**The `READY TO MERGE` label is added only when: the stable `ci-gate` check is present and green
on the head commit, no check has failed or is in progress, the PR is mergeable, no review
requests changes, unresolved review threads are zero, no DeepSeek review has unread findings in
its body (they are closed with the `DEEPSEEK_REVIEW_READ` label) and the DeepSeek review is
definitively finished (clean verdict, rounds exhausted or `NO_DEEPSEEK_REVIEW` exemption).**

The rule has a single implementation, `cmd_pr_status_json` in `scripts/gh-automation.sh`, used
by `pr-labeler.yml` and by `pr-status`. Every condition fails closed: an unreadable count is
`-1`, never `0`.

"Green" is not "not red". A PR **with no checks at all** satisfies "zero failed, zero in
progress", and it is a reachable state: a conflicting PR has no merge ref and therefore runs no
`pull_request` workflow. That is why the helper requires `ci-gate` **present and green** and
`mergeable == MERGEABLE`.

The labeler runs on `workflow_run` when CI or the DeepSeek review finish, plus a sweep every
six hours for transitions without an event (resolution of the last thread, mergeability
recomputed after a merge into the base). `workflow_dispatch` or `/process-pr` force a pass. Do
not add a `pull_request` trigger to the labeler.

**A failed label write makes the labeler fail.** `pr-label` exits non-zero if the write does not
land, and `pr-check-label` answers **0 present / 1 absent / 2 undeterminable**. A red labeler run
means a write failed: read the log.

#### The thread count only sees half of a review

DeepSeek delivers findings in two forms. Inline comments create **threads**, which the rule
counts. General comments sit in the review **body** and create no threads: a review made only of
those counts `0 unresolved` despite having real findings. That is why `pr-status-json` also
counts `deepseek_unread_findings`. The clean verdict is recognised by the
`<!-- deepseek:no-findings -->` marker at the start of the body, without
`<!-- deepseek:full-review -->`.

**The acknowledgement is the `DEEPSEEK_REVIEW_READ` label, and it is dated**: only reviews
submitted before the label was applied count as read. A second forced review blocks again;
removing the label cancels the acknowledgement. A human may apply it after reading the findings.
The AI only after reading **every** finding, fixing the valid ones or rejecting the others with
evidence, and posting the replies on the PR; its write must be newer than the reviews it
acknowledges. `NO_DEEPSEEK_REVIEW` stays human-only.

### Completion-Driven Iteration Lifecycle

There is no calendar and milestones have no due date. `iteration-lifecycle.yml` runs when an
issue with a milestone or with the `ceremony` label is closed, finds the active milestone and
evaluates it:

1. As long as the active iteration has open issues, it does nothing.
2. When the last issue is closed, it creates a backlog refinement ceremony for that milestone.
3. Closing that ceremony creates an iteration planning ceremony.
4. `/scrum-master iteration-plan` picks a batch of work, creates and populates the next
   `Iteration N` milestone, then closes the completed one.

**There can be several open iterations**: the active one is the open milestone with the
**lowest** sequence (from `Iteration N` / `Sprint N`, otherwise from the milestone number),
compared as a number. Two open milestones with the same lowest sequence make the run fail.

**Cold start**: with no milestone at all, `iteration-advance` does nothing. The first
`/scrum-master iteration-plan` creates `Iteration 1` directly.

### Automated PR Review (DeepSeek)

`deepseek-pr-review.yml` reviews the code with the DeepSeek API (`deepseek-flash`,
`reasoning_effort=high`):

| Mode | Trigger | Behaviour |
|------|---------|-----------|
| **A — Full review** | PR opened or new commit | Reads the whole diff and posts inline comments with suggestions. A finding that names a file must be anchored to that file; the body is reserved for observations that do not belong to a single file. |
| **B — Reply** | Reply to a DeepSeek comment | Reads the diff and the thread, replies in the same thread. Never touches the code. |

**No approval**: GitHub does not allow Actions to approve PRs. The clean verdict is a COMMENT
review that starts with `<!-- deepseek:no-findings -->`.

**Round limit**: automatic full reviews are at most **1** per PR (`MAX_ROUNDS`). After that,
pushes start no further reviews and a notice is posted once. Replies (Mode B) continue without a
limit. To force another pass:
`bash scripts/gh-automation.sh pr-deepseek-force-review <pr> [ref]` (`ref` defaults to `develop`).
With `--measure-only` it re-reviews a diff without publishing anything.

**Which copy of the configuration applies**: `pull_request` runs use the merge ref, so they run
the workflow and scripts of the **base branch**.

**Diff limit: 90,000 characters** (`DEEPSEEK_MAX_DIFF_CHARS`, measured in voxelheim-v2 with
seventeen replays at `high`). Beyond it, the diff is truncated, every unread file is named in the
log **and in the review as a finding**, and the PR stays blocked until someone acknowledges it.
A truncated review cannot come back clean. Splitting the PR is the reliable fix; better still,
size the issue beforehand (see `/scrum-master` and `/dev-issue` Step 5). The limit and the effort
are a single measurement: changing either makes `deepseek-budget.test.sh` fail until the limit
is re-measured.

Generated code (`gen/` folders, `*_generated.*`) and lockfiles (`Cargo.lock`, `pnpm-lock.yaml`,
`go.sum`, by exact name) are excluded from the diff. Manifests (`Cargo.toml`, `package.json`)
stay: a new dependency is exactly what a reviewer needs to see.

**Budget and timing**: `DEEPSEEK_MAX_OUTPUT_TOKENS=384000`; the request budget is
`DEEPSEEK_REQUEST_TIMEOUT_SECONDS` × (`DEEPSEEK_MAX_RETRIES` + 1) = 2700s × 2 = 90 minutes, and
the job has `timeout-minutes: 100`. **The budget must stay below the job limit**:
`scripts/test/deepseek-budget.test.sh` verifies it. If the job limit wins, the step ends up
`cancelled` with no output and `pr-status-json` counts `CANCELLED` as failed.

**An unreadable diff makes the run fail** (fetch error, files without a patch, network errors),
and an unreadable round count is not worth zero.

**Security**: DeepSeek creates no commits, does not push and does not modify files. It ignores
its own comments. It never claims to have taken actions its script cannot take (such as
resolving a thread).

### Setup Prerequisites

0. **Local tools**: `gh` (2.18.0 or later) and `jq`, see the table in `README.md`.
   `scripts/gh-automation.sh` requires both.
1. **GitHub token**: a fine-grained PAT with `contents: read/write`, `pull_requests: read/write`,
   `issues: read/write` on this repository, saved as the `GH_PIPELINE_TOKEN` secret. The labeler
   and the iteration lifecycle need it (writes made with `GITHUB_TOKEN` do not trigger other
   workflows).

   **This token does not read CI status, and no permission fixes that**: the Checks API is
   GitHub App-only. `pr-labeler.yml` passes `GH_CI_TOKEN: ${{ secrets.GITHUB_TOKEN }}` with
   `checks: read`, `statuses: read` and `actions: read`, and `gh_ci()` routes that single read
   there. **Every new caller that reads checks must use `gh_ci`, not `gh`.**
2. **DeepSeek API key**: `DEEPSEEK_API_KEY` secret (`Settings → Secrets and variables →
   Actions`).
3. **Merge protection**: two rulesets on `develop` and `main`, see
   `.github/branch-protection.md`. The second one (`ci-gate` required) is enabled only after
   `ci-gate` has run on a real PR.
4. **Labels**: `READY TO MERGE`, `needs-review`, `needs-work`, `DEEPSEEK_REVIEW_READ`,
   `NO_DEEPSEEK_REVIEW`, `ceremony`, `feature`, `bug`, `needs-refinement`, `needs-triage`,
   `ready-for-dev`, `needs-privacy-review`.

## Definition of Done

**Every PR opened by the pipeline MUST satisfy ALL of this before it is created:**

- [ ] **Formatting** — `cargo fmt --all --check` (in `src-tauri/`, the whole workspace)
- [ ] **Lint** — `cargo clippy --all-targets --locked -- -D warnings`
- [ ] **Build** — `cargo build --locked` and `pnpm build` (`tsc && vite build`)
- [ ] **Tests** — `cargo test --locked` and `pnpm test` (`node --test`, tests of the frontend's pure functions)
- [ ] **Automation** — `scripts/test/*.test.sh` and `python3 .github/scripts/test_deepseek_review.py`
- [ ] **No debug prints** in production paths
- [ ] **No secrets** — no keys, tokens, passwords or credentials in code or tests
- [ ] **System access only in the backend** — and no writes to sysfs, no root privileges
- [ ] **Clean publication privacy** — no real email, personal data, machine path, hostname or
      private endpoint; `scripts/check-publication-privacy.sh` passes on the tree and
      `scripts/check-commit-privacy.sh` on the commits the branch adds

If a gate fails, the implementation is not finished.

### What CI enforces

`.github/workflows/ci.yml` runs:

| Job          | Gate |
| ------------ | ---- |
| `detect`     | Privacy checks on the tree and on the PR's commits, then classifies the changed files with `scripts/changed-areas.sh` |
| `backend`    | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo build --locked`, `cargo test --locked` (with Tauri's system libraries) |
| `frontend`   | `pnpm install --frozen-lockfile`, `pnpm build`, `pnpm test` |
| `automation` | The whole `scripts/test/*.test.sh` suite plus `test_deepseek_review.py` |
| `ci-gate`    | The only stable check: verifies that every selected job succeeded and every skip was authorised |

On a PR targeting a base other than `main`, `detect` selects jobs from the diff: `src-tauri/**`
triggers `backend`, `src/**` and the frontend toolchain files trigger `frontend`,
`package.json`/`pnpm-lock.yaml` trigger both, `ci.yml` triggers everything, documentation
triggers nothing. `automation` always runs. An unrecognised path or an unreadable diff triggers
everything (fails open). On a PR targeting `main` everything runs.

Two traps:

1. **Never `paths:` filters at workflow or job level.** A filtered workflow produces no result
   that `ci-gate` can verify. The right tool is a job-level `if:`.
2. **`cancel-in-progress` produces CANCELLED jobs, which `pr-status-json` counts as failed.** In
   `ci.yml` cancellation applies only to `pull_request` events, where the cancelled run always
   belongs to a commit that is no longer the head.

### Release

`.github/workflows/release.yml` starts on a `v*` tag, accepted **only on a commit already in
`main`** (and therefore passed through a PR with a green `ci-gate`): the `verify` job checks this
before building. Then it builds the `.deb` on `ubuntu-latest` and creates a **draft release**; if
the draft already exists it replaces its package, while an already published release is never
touched. Publishing stays human. Only the job that creates the draft has `contents: write`. The
package is not signed. How LACT and GPU Screen Recorder reach the user is in
`docs/decisions.md` (distribution).

Actions are pinned by full SHA with the release label, and checkout does not persist
credentials (`scripts/test/actions-hardening.test.sh`). The Rust toolchain is pinned in
`src-tauri/rust-toolchain.toml`, aligned with `dtolnay/rust-toolchain` in CI.
