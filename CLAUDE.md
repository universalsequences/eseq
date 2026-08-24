# Project Instructions for AI Agents

This file provides instructions and context for AI coding agents working on this project.

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:1105d646 -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/core-concepts/sync-concepts.md for details and anti-patterns.

## Agent Context Profiles

The managed Beads block is task-tracking guidance, not permission to override repository, user, or orchestrator instructions.

- **Conservative (default)**: Use `bd` for task tracking. Do not run git commits, git pushes, or Dolt remote sync unless explicitly asked. At handoff, report changed files, validation, and suggested next commands.
- **Minimal**: Keep tool instruction files as pointers to `bd prime`; use the same conservative git policy unless active instructions say otherwise.
- **Team-maintainer**: Only when the repository explicitly opts in, agents may close beads, run quality gates, commit, and push as part of session close. A current "do not commit" or "do not push" instruction still wins.

## Session Completion

This protocol applies when ending a Beads implementation workflow. It is subordinate to explicit user, repository, and orchestrator instructions.

1. **File issues for remaining work** - Create beads for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **Handle git/sync by active profile**:
   ```bash
   # Conservative/minimal/default: report status and proposed commands; wait for approval.
   git status

   # Team-maintainer opt-in only, unless current instructions forbid it:
   git pull --rebase
   git push
   git status
   ```
5. **Hand off** - Summarize changes, validation, issue status, and any blocked sync/commit/push step

**Critical rules:**
- Explicit user or orchestrator instructions override this Beads block.
- Do not commit or push without clear authority from the active profile or the current user request.
- If a required sync or push is blocked, stop and report the exact command and error.
<!-- END BEADS INTEGRATION -->


## Build & Test

Prefer `cargo nextest run` and select the narrowest exact test that validates a
change. Full workspace runs are reserved for explicitly exhaustive work such as
eseq-4tl; see `AGENTS.md` for selection examples and runtime policy.

### DGenLisp compiler (fetched, not tracked)

The DGenLisp compiler binary is not in git. `content/dgenlisp.lock` pins the
published distribution per target; run `./scripts/fetch_dgenlisp.sh` once per
fresh checkout (idempotent, sha256-verified) to install it under
`crates/sequencer/tools/` (gitignored). Anything that needs the compiler and
cannot find it hard-fails naming that command. `ESEQ_DGENLISP_TOOL=/abs/path`
overrides it with a locally built compiler.

### Cheap clean-HEAD check

Do not stash and do not cold-clone the repository to determine whether one test
fails at HEAD. Reuse an isolated worktree and a dedicated target directory:

```bash
wt=/tmp/eseq-head-test; target=/tmp/eseq-head-test-target
[ -e "$wt/.git" ] || git worktree add --detach "$wt" HEAD
git -C "$wt" checkout --detach HEAD
(cd "$wt" && \
  CARGO_TARGET_DIR="$target" \
  cargo nextest run -p <package> -E 'test(=<fully-qualified-test-name>)')
```

The dedicated target directory keeps Cargo artifacts from the clean checkout
separate from working-checkout artifacts; sharing a target directory between
worktrees can make Cargo run a binary built from the wrong source tree. The
worktree is disposable and isolated, so resetting it never touches the working
checkout. Remove it with `git worktree remove /tmp/eseq-head-test` when it is no
longer useful; remove `/tmp/eseq-head-test-target` too if its cached artifacts
are no longer needed.

### Test stack budget

`.cargo/config.toml` applies one 16 MiB `RUST_MIN_STACK` budget automatically to
Cargo-launched test processes. The same number is
`sequencer::REQUIRED_THREAD_STACK_SIZE` for explicitly spawned scheduler/UI test
threads. Do not add local 32/64 MiB literals or rely on a remembered shell
prefix.

LLDB investigation for eseq-4tl found that debug overflows while loading the UI
run through recursive `Expression::clone` and then
`Compiler::compile_expression -> compile_list -> compile_if_statement /
compile_let_statement / compile_function`. Expression cloning is now iterative.
Compiler traversal is still proportional to authored Lisp nesting and is
tracked as `eseq-4tl.1`; release builds load the checked-in UI on a normal stack,
but adversarially deep user source remains a production crash risk until that
bead is complete. With the configured budget, any remaining overflow is
isolated by nextest and reported as the named test rather than aborting a shared
test binary.

As of 2026-08-20 there are no known pre-existing failures: full debug is 4,272
passed / 32 skipped and full release is 4,270 passed / 32 skipped. Commands and
timings are recorded in `docs/test-suite-performance.md`.

## Architecture Overview

_Add a brief overview of your project architecture_

## Conventions & Patterns

_Add your project-specific conventions here_
