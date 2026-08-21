# Metal UI

This directory contains the declarative ESeqLisp UI used by `metal_seq`. The
Rust host and event loop live in `../src/ui/`; the retained terminal interface
lives separately in `../src/tui/`.

- `main.lisp` is the root UI manifest and layout entrypoint.
- `effects.lisp` loads the effect/instrument panel modules in `effects/`.
- `builtin-effects.lisp` defines the load order for built-in effect panels
  under `effects/builtin/`.
- `themes/` contains the selectable application themes.
- `capture-fixtures/` contains deterministic projects for headless Metal
  rendering and visual review.

Loads use the explicit `@/` working-directory prefix (for example,
`@/ui/effects.lisp`). That keeps their canonical paths stable during startup,
hot reload, tests, and headless capture, regardless of which UI module issued
the load.
