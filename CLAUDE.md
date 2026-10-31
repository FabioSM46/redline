# CLAUDE.md

Read and follow the instructions in @AGENTS.md at the root of the project.

The repository is **public**: everything published stays readable forever. Never put a real email
address, personal data, machine username or path, private hostname, secret or credential in a
tracked file, commit, branch, PR, review, CI log or artifact. Use the GitHub `noreply` address,
reserved example values and placeholders such as `<repo-root>`. Before every push run
`bash scripts/check-publication-privacy.sh <base> <head>` on the content (the index and every
commit of the branch) and `bash scripts/check-commit-privacy.sh <base> <head>` on authors and
messages; before publishing an issue or PR body, `bash scripts/check-body-privacy.sh`.

Agents never read `.env` files. No tool signatures in commits and PRs: no agent
`Co-Authored-By` trailer, no "Generated with", no `claude.ai/code/session_…` link and no
`Claude-Session:` trailer.

Everything in or published from the repository is written in English, even when the conversation
is in another language: code, comments, user-facing strings, documentation, skills, issues, PRs,
review comments, commit messages, branch names, labels, milestones and release notes.

Quick reference — the pipeline skills:

- `/dev-issue <number>` — implements a GitHub issue end to end (worktree → gates → PR)
- `/process-pr [number]` — forced pass over an open PR (DeepSeek feedback + CI fixes)
- `/develop-iteration [iteration]` — delivers the issues of a milestone (parallel agents → PRs → merge)
- `/scrum-master feature-spec | backlog-refine | iteration-plan` — scrum ceremonies

Rules that override everything else: work only in git worktrees, start from `develop`, never push
or merge to `main`, never read `.env` files, system access only in the backend, Redline never runs
as root and never writes to sysfs (tuning goes through `lactd` with confirmation), GPU Screen
Recorder only as an external process, no new dependency without an explicit request. A failed
privacy check blocks the push even when build and tests are all green.
