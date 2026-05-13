# AGENTS.md

## Instrument testing

Use `instrument_probe` when changing DGenLisp instruments, wavetable/tensor loading, or host-side instrument initialization. It exercises the same host compile/load/init path as the app and gives quick signal checks without launching the UI.

Example:

```sh
cargo run --bin instrument_probe -- emulations/monomachine-dpro-wave-v2 \
  --frames 4096 \
  --min-peak 0.01 \
  --min-rms 0.001
```

For saved instrument names, the probe resolves local assets from the saved instrument directory. For direct file paths, it resolves assets from the source file's parent directory. Use `--param name=value` for parameter overrides and `--json` for machine-readable output.

## UI/layout testing

When changing Lisp UI structure or widget wrapper argument order, do not stop at parse/render-tree tests. Add or run a layout test that proves expected text/debug nodes have finite, nonzero measured rects inside the visible panel; regressions often look like "the panel exists, but its children measured to zero or disappeared."

>> EXTREMELY IMPORTANT <<<
NO HACKS. The user is EXTREMELY concerned about code quality, much more so than immediate results. If they ask you to build something and, while doing so, you hit a wall, and realize that the only way to ship the requested feature is to
IMMEDIATELLY. Either fix the underlying flaw that blocked you in a ROBUST, WELL
DESIGNED, PRODUCTION READY manner, or be honest that the prompt can't be completed without hacks.
To make it very clear:
- DO NOT INTRODUCE HACKS IN THE CODEBASE.
- DO NOT COMMIT CODE THAT COULD BREAK THINGS LATER.
- DO NOT COMMIT PARTIAL SOLUTIONS OR WORKAROUNDS.
THIS IS VERY IMPORTANT.
THIS IS VERY IMPORTANT.
THIS IS VERY IMPORTANT.
The author appreciates honestly and he WILL be glad and thankful if you respond
a request with "I couldn't complete your request because the repository lacked support for X". He will be even happier if you go ahead and update the repo to provide the necessary support in a well designed, robust way. But he will be VERY ANGRY if, while attempting to implement a feature, you introduce a workaround that will potentially break things later.
NEVER introduce hacks in the codebase.
Also assume that none of the code you're working in is in production, so, backwards compatibility is NOT IMPORTANT. If you find something that is poorly designed and fixing it would require breaking existing APIs or behavior, DO SO.
Do it properly rather than preserving a flawed design. Prioritize clarity,  correctness, and maintainability over compatibility with existing code.
Core values:
- ABSOLUTE code quality over speed of delivery.
- Correctness over convenience.
- Clarity over cleverness.
- Maintainability over short-term productivity.
- Robust design over quick fixes.
- Simplicity over complexity.
- Doing it right over doing it now.
- Honesty above everything.
-  After every change you make, provide a clear, honest report on ANY change that you are not confident about and that could be considered a fragile hack.
