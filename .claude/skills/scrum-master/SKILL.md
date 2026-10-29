---
name: scrum-master
description: Scrum ceremony facilitator. Use for backlog refinement, iteration planning, and feature specification.
argument-hint: <ceremony>
allowed-tools: Bash(gh *) Bash(git *) Bash(bash *) Bash(cat *) Bash(xargs *) Bash(date *) Bash(echo *) Bash(set *) Bash(jq *)
---

# scrum-master — scrum ceremonies

Trigger: `/scrum-master <ceremony>`, where the ceremony is one of:
- `backlog-refine` — backlog refinement
- `iteration-plan` — planning of a completion-driven iteration
- `feature-spec` — feature → technical specification → issues

## Purpose

Manages the repository's development cycle. The user brings the ideas; this skill listens,
writes the technical specifications, creates the issues and organizes the backlog and
iterations.

## Ceremonies

### 1. backlog-refine

Trigger: `/scrum-master backlog-refine` or from a ceremony issue.

**Workflow:**

1. Read all open issues:
   ```bash
   gh issue list --state open --json number,title,labels,createdAt,updatedAt --limit 100
   ```

2. Exclude ceremony issues (label `ceremony`).

3. Classify every issue against the same required-field contract that `/dev-issue` enforces
   (Area, Type, Priority, User Story, Acceptance Criteria, Technical Context, Out of Scope),
   reading **only the issue body**: it is all that `/dev-issue` sees.
   - **Ready**: all seven fields present and substantive in the body
   - **Needs spec**: a required field is missing or is only a placeholder
   - **Needs triage**: new or unclassified
   - **Stale**: no activity for 30+ days → propose closing or redefining it

4. For every **Needs spec** issue:
   - read the body;
   - write the missing acceptance criteria starting from the user story;
   - write the missing technical context starting from the area and the existing code;
   - write **Out of Scope** if missing, with concrete files/modules/behaviors, never "unrelated
     changes": it is `/dev-issue`'s only defense against scope creep;
   - **write every field into the issue body. A comment is not enough.** `/dev-issue` reads the
     issue with `gh issue view <number> --json title,body,labels` and derives the fields only
     from the body, it never reads comments.

     `gh issue edit --body-file` replaces the **whole** body, so the edit must be surgical by
     discipline: replace only the `### <Field>` section and reproduce every other byte. Read,
     rewrite, replace, then reread the body and verify that the only difference is that section
     (GitHub adds a single trailing newline).

     ```bash
     gh issue view <number> --json body --jq '.body' > /tmp/issue-<number>.md
     # Rewrite the file with the section replaced — a heredoc
     # `cat > /tmp/issue-<number>.md <<'BODY' … BODY`, because this skill's allowed-tools
     # include `cat` and no file-editing tool — then:
     gh issue edit <number> --body-file /tmp/issue-<number>.md
     gh issue view <number> --json body --jq '.body'   # reread and compare
     ```

   - Also add a comment when a finding has to reach a person through notifications (an ordering
     risk, a dependency between issues). The comment is the trail, the body edit is the
     delivery. Never only the comment.

5. Reorder priorities based on:
   - dependency chains (backend module before the Tauri command, command before the UI);
   - value (monitoring and replay before tuning, tuning before polish);
   - effort (alternate quick wins and large pieces of work).

6. Update the labels when an issue is ready: `needs-refinement` → `ready-for-dev` only when all
   seven fields are present and substantive **in the body**. `iteration-plan` picks work from
   `ready-for-dev`, so a label earned with a comment schedules work on which `/dev-issue` would
   stop at the first step.

7. Post a summary comment on the ceremony issue (or in a new issue if started by hand) with:
   - total open issues;
   - breakdown by category;
   - suggested priority order for the next iteration;
   - issues proposed for closing.

8. Close the ceremony issue.

### 2. iteration-plan

Trigger: `/scrum-master iteration-plan` from the ceremony issue created by
`iteration-lifecycle.yml` for the milestone.

**Cold start (new repository)**: if NO iteration milestone exists, open or closed, there is no
milestone to check and no ceremony to wait for. Skip steps 1–4: directly create the undated
milestone `Iteration 1`, populate it from `ready-for-dev` (steps 5–8) and post the plan. After
the first iteration everything goes through the completion-driven state machine.

**Workflow:**

1. Identify the active iteration: the open milestone with the **lowest** sequence, read from an
   `Iteration N` / `Sprint N` title and failing that from the milestone number, the same rule as
   `iteration-advance`. Fail closed if no milestone is open (outside the cold start) or if two
   open milestones share the **lowest** sequence. A collision higher up does not count until it
   becomes the lowest.
   ```bash
   REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
   gh api "repos/$REPO/milestones?state=open&per_page=100"
   ```

   **Several open milestones are an expected state.** Planning ahead is the point: a coherent
   block of work can be assigned to a future iteration while the current one is still in
   progress. Creating and populating an iteration is always possible; **closing** a completed
   milestone is what steps 2 and 3 control.

2. **To be closed, the completed milestone must have zero open issues.** Iterations are
   completion-driven: never move unfinished work automatically and never close a milestone that
   still has open issues. This applies only to step 9.

3. **Closing it also requires its refinement.** Read the
   `<!-- iteration-lifecycle:iteration-plan:milestone-N -->` marker of the planning ceremony and
   require exactly one closed backlog-refinement issue with the matching marker
   `<!-- iteration-lifecycle:backlog-refine:milestone-N -->`. Refuse to close if the refinement
   is missing, open or duplicated.

4. Identify the target sequence: **the one in the ceremony title, one above the active
   iteration.** With planning ahead it may already exist:

   - **it exists and contains work**: the batch was chosen when it was planned. Steps 5–7 have
     nothing to select and step 8 creates nothing; step 9 remains.
   - **it does not exist, or exists empty**: this is the normal case, select the work and create
     it at step 8.

   **Never create a second milestone on a sequence that already has one.** To plan further
   ahead, take the sequence above the **highest** existing one, open or closed. Do not change
   milestone state yet.

5. Read the issues in priority order from the completed refinement or from the priority labels.

6. Select the issues for the iteration:
   - a realistic batch for a single developer;
   - balanced between backend and frontend when the backlog allows it;
   - at least one bug or piece of technical debt if there are any;
   - respect every blocking dependency;
   - at least one selected issue; if none is ready, explain the blocker and leave the ceremony
     and the completed milestone open. This applies only if step 4 identified a milestone to
     create or to add work to.

7. For every selected issue:
   - estimate: S (hours), M (1–2 days), L (3–5 days);
   - check the blocking dependencies;
   - suggest an implementation order.

8. Create the undated milestone for the step 4 sequence (or `Iteration 1` at cold start), assign
   every selected issue to it and verify the assignments before closing the completed milestone.
   This order leaves the previous milestone recoverable if creation or assignment fails. If the
   target milestone already exists, assign the work there.
   ```bash
   for issue in <issue-numbers>; do
     gh issue edit "$issue" --milestone "Iteration <number>"
   done
   ```

9. Close the completed milestone only after the new iteration contains the chosen work, and only
   if steps 2 and 3 are satisfied. If the active iteration still has open issues, leave it open.
   (Cold start: nothing to close.)

10. Post the plan as a comment with:
    - iteration goal (one sentence);
    - list of issues with estimate and area;
    - total estimated effort;
    - dependencies and risks.

11. Close the ceremony issue. Closing it reruns the lifecycle helper, which should see the new
    active iteration and change nothing.

### 3. feature-spec

Trigger: `/scrum-master feature-spec <feature-description>`

**Workflow:**

1. Compare the description with the project documents: `docs/context.md` (why Redline exists
   and what it takes from Adrenalin and what it leaves out), `docs/roadmap.md` (milestones),
   `docs/architecture.md` (planned modules) and `docs/decisions.md` (decisions made and open).
   Ask questions if:
   - the area is not stated and not obvious;
   - the scope is ambiguous;
   - the dependencies are unclear;
   - the feature contradicts a recorded decision (say which one, and ask).

2. Write a technical specification (in the conversation):
   - **Summary**: one paragraph about the feature
   - **Areas involved**: backend, frontend or both
   - **Backend (Rust/Tauri)**: modules in `src-tauri/src/`, exposed Tauri commands, types
     serialized to the frontend, disk I/O, error handling
   - **Frontend (React/TypeScript)**: components, state, search/filters/sorting, `invoke()`
     calls, following the rules of the "Interface" section of `AGENTS.md` (minimal, modern UI);
     every frontend issue repeats the constraint in its Technical Context
   - **External tools** (if needed): `lactd` commands, GPU Screen Recorder options and signals,
     sysfs files read; to be verified against the documentation or the source of the pinned
     version
   - **Data and persistence** (if needed): local state, format, migrations
   - **Constraints**: system access only in the backend; Redline never as root and never writing
     to sysfs (tuning goes through `lactd` with confirmation); GPU Screen Recorder only as an
     external process; missing sensors or tools handled as a disabled feature, not as an error;
     no new dependencies without discussing them
   - **Risks**: what can go wrong, what needs attention

3. Get the specification approved. Do NOT create issues until the user confirms.

4. After approval, create the issues in the feature template format.

   **The issue body is a contract with `/dev-issue`.** That skill reads these headings and treats
   Area, Type, Priority, User Story, Acceptance Criteria, Technical Context and Out of Scope as
   required, like `.github/ISSUE_TEMPLATE/feature_request.yml`. An issue missing one of them
   blocks the pipeline at the first step. Write **all seven**, always.

   The dropdown values are fixed strings; use them verbatim:

   | Field | Allowed values |
   |-------|----------------|
   | Area | `backend (Rust / Tauri)` · `frontend (React / TypeScript)` · `shared (Cross-cutting)` |
   | Type | `feature (New functionality)` · `enhancement (Improvement to an existing feature)` · `refactor (Restructuring with no behavior change)` |
   | Priority | `high (Blocks other work)` · `medium (Normal priority)` · `low (Useful but not urgent)` |

   For every unit of work:

   ```bash
   gh issue create \
     --title "[Feature]: <component description>" \
     --body "$(cat <<'EOF'
   ### Area

   <one of the three allowed values, verbatim>

   ### Type

   <one of the three allowed values, verbatim>

   ### Priority

   <one of the three allowed values, verbatim>

   ### User Story

   As a <role>, I want <goal>, so that <reason>

   ### Acceptance Criteria

   - [ ] <criterion 1>
   - [ ] <criterion 2>

   ### Technical Context

   <relevant files, patterns and constraints from the specification>

   ### Out of Scope

   - <what this issue must NOT touch>

   ### Code Pointers

   - `src-tauri/src/commands.rs:12` — <why it matters>

   ### Dependencies

   <blocking issues, or "None">

   ### Test Strategy

   <what to test and at what level>
   EOF
   )" \
     --label "feature" \
     --label "needs-refinement"
   ```

   **Out of Scope is the field with the most leverage.** It is the only boundary `/dev-issue`
   has against scope creep, and the one most often left vague. Name concrete files, modules or
   behaviors.

   Bugs use `.github/ISSUE_TEMPLATE/bug_report.yml`, which requires Area, What happened?, What
   should have happened?, Steps to reproduce, Technical Context and Out of Scope, without
   Type/Priority/User Story.

5. Group the issues:
   - **foundations first**: backend types and modules, data formats, source clients;
   - **then the core**: Tauri commands and the main UI flows;
   - **finally the polish**: edge cases, error handling, visual feedback.

6. Report to the user:
   - number of issues created;
   - suggested implementation order;
   - total estimated effort;
   - link to the specification.

## Sizing: an issue is written against the review limit

`DEEPSEEK_MAX_DIFF_CHARS` is **90,000** characters. A PR over the limit is truncated, every
unread file becomes a finding and the PR stays blocked until someone acknowledges it.
`/dev-issue` Step 5 splits an issue that is too large into parts, but that is a remedy: when
`/dev-issue` reads the issue, its shape is already fixed. This is the moment a boundary is
chosen with no code written yet.

### Estimate every issue you write or refine

Count, instead of guessing:

- the **areas** (backend, frontend);
- the **surface kinds** the issue names: a mechanism, a data model, a Tauri command, a UI screen
  or control, persistent state, a client for an external source. One kind is one PR; two kinds
  are normally two PRs;
- the **acceptance criteria**: past six or seven, the groups they fall into are the boundaries;
- the multipliers: tests are worth about two thirds of a change with real coverage.

Two symptoms that there is more than one part, readable while you write the body:

- **the summary needs an "and" between two nouns of different kinds** (a parser *and* a screen,
  a client *and* the UI that uses it);
- **more than six or seven acceptance criteria.**

### What to do when an issue is too large

**Splitting the issue is better.** Two issues, each one PR, each with its own criteria and its
own `Dependencies` line, are better than one issue and a note: the milestone counts the real
work and `/dev-issue` works without having to decide on its own.

**When the work really is a single issue**, keep it whole and write the boundary in the body
under a `Suggested split` heading, with every part, what it contains and the merge order.

**Never split on a character count.** The boundary is one the code already draws.

### Write the axes of the estimate, not the verdict

Put the estimate and the reasoning in the body (or in the refinement comment): areas counted,
surface kinds counted and named, multipliers, and a band stated as a **minimum, not as a
count**. Do not write "One PR" as a verdict: an agent that reads a number already written
inherits it instead of redoing the estimate, and the decision is made by whoever is furthest
from the code.

## Guardrails

- Never create issues without the user confirming the specification.
- Never modify closed issues or milestones.
- Respect the areas: no backend issues for a frontend-only feature.
- A feature that brings data from the backend to the frontend ALWAYS has a backend part (Tauri
  command and serialized types): make it explicit instead of hiding it in a UI issue.
- **Size every issue against the 90,000-character limit before assigning it to a milestone.**
- When in doubt about scope, ask.
- **Every issue body and every comment goes through `bash scripts/check-body-privacy.sh` before
  being published** (for example `bash scripts/check-body-privacy.sh < /tmp/issue-<number>.md`):
  no real emails, local machine paths or private network addresses. In code references use
  repository-relative paths.
- Ceremony issues (label `ceremony`) are excluded from all reports.

## References

- Context, roadmap, architecture, decisions: `docs/`
- Issue templates: `.github/ISSUE_TEMPLATE/`
- Pipeline: `AGENTS.md`
- Review limit and its costs: `AGENTS.md`, "Automated PR Review (DeepSeek)"; `/dev-issue` Step 5
- Helper: `scripts/gh-automation.sh`
