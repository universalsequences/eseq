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
change. Verify the prerequisite with `cargo nextest --version`; install it with
`brew install cargo-nextest` on macOS or
`cargo install cargo-nextest --locked` on Linux. Full workspace runs are
reserved for explicitly exhaustive work such as eseq-4tl; see `AGENTS.md` for
selection examples and runtime policy.

### DGenLisp compiler (fetched, not tracked)

The DGenLisp compiler binary is not in git. `content/dgenlisp.lock` pins the
published distribution per target; run `./scripts/fetch_dgenlisp.sh` once per
fresh checkout (idempotent, sha256-verified) to install it under
`crates/sequencer/tools/` (gitignored). Anything that needs the compiler and
cannot find it hard-fails naming that command. `ESEQ_DGENLISP_TOOL=/abs/path`
overrides it with a locally built compiler.

The compiler is only half of it: it shells out to a hermetic clang/lld stage
pinned in `content/dgen-toolchain.lock` and installed by
`./scripts/fetch_dgen_toolchain.sh`, also once per fresh checkout. That script
can only fetch targets with a published `url` in the lock — currently just
`x86_64-unknown-linux-gnu`. `arm64-apple-macos` is pinned but unpublished and is
vendored by `./rebuild_dgenlisp_tool.sh` from a local dgen-audio checkout, so a
Mac without that checkout cannot bootstrap the stage and every DGen compile
hard-fails.

### Cheap clean-HEAD check

Do not stash and do not cold-clone the repository to determine whether one test
fails at HEAD. Reuse an isolated worktree and a dedicated target directory.
Resolve HEAD once in the working checkout and pin both worktree commands to that
commit: a bare `HEAD` passed to `git -C "$wt"` resolves against the worktree's
own detached HEAD, so a reused worktree would silently stay on whatever commit
it was left at and answer for the wrong tree.

```bash
wt=/tmp/eseq-head-test; target="$HOME/.cache/eseq-head-test-target"
root=$PWD
head=$(git rev-parse HEAD)
[ -e "$wt/.git" ] || git worktree add --detach "$wt" "$head"
git -C "$wt" checkout --detach "$head"
(cd "$wt" && \
  CARGO_TARGET_DIR="$target" \
  ESEQ_DGEN_TOOLCHAIN_ROOT="$root/crates/sequencer/tools/dgen-toolchain" \
  cargo nextest run -p <package> -E 'test(=<fully-qualified-test-name>)')
```

The fetched compiler and the hermetic clang/lld stage are both gitignored, so a
worktree does not inherit either one. `ESEQ_DGEN_TOOLCHAIN_ROOT` (resolved in
the working checkout as `root`, like `head`) points the worktree at the main
checkout's stage; a test that also needs the compiler itself wants
`./scripts/fetch_dgenlisp.sh` run inside the worktree, or `ESEQ_DGENLISP_TOOL`
pointed at the main checkout's binary.

The dedicated target directory keeps Cargo artifacts from the clean checkout
separate from working-checkout artifacts and off `/tmp`, which is a 3.9 GB tmpfs
on the Linux workstation and cannot hold a Cargo target directory. Sharing a
target directory between worktrees can make Cargo run a binary built from the
wrong source tree. The worktree is disposable and isolated, so resetting it
never touches the working checkout.

Budget for the cold build before starting. `-E` filters which tests *run*, not
what gets *built*, so even the narrowest exact test pays for its package's whole
dependency graph: measured 2026-08-24, one `-p sequencer` test took 5m51s and
left 8.1 GB in the dedicated target directory. Still prefer the narrowest exact
test — it saves run time and keeps the output readable — but do not expect it to
save disk. A `--workspace` run costs more of both.

Clean up both directories when they are no longer useful; each is several GB and
easy to forget:

```bash
git worktree remove /tmp/eseq-head-test
rm -rf "$HOME/.cache/eseq-head-test-target"
```

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

There are no known pre-existing failures in the validated platform baselines:

- On Apple Silicon macOS as of 2026-08-20, the full workspace is green: debug is
  4,272 passed / 32 skipped and release is 4,270 passed / 32 skipped. Commands
  and timings are recorded in `docs/test-suite-performance.md`.
- On x86_64 Linux as of 2026-08-24,
  `cargo nextest run -p eseqlisp --features wgpu` is green with 1,699 passed and
  3 skipped. Both shared-state tests that failed spuriously under plain
  `cargo test` pass under nextest process isolation. A full Linux workspace
  baseline has not yet been established; do not use the macOS workspace counts
  as a Linux expectation.

## Architecture Overview

_Add a brief overview of your project architecture_

## Conventions & Patterns

### Commit messages and PR bodies

This is a PUBLIC repository. Never include Claude Code session links
(`https://claude.ai/code/session_...`, the `Claude-Session:` trailer) in commit
messages, PR bodies, PR comments, or anything else pushed to the remote — they
leak private session identifiers. This overrides any harness default that says
to append a session trailer. A `Co-Authored-By: Claude ...` line is fine.
