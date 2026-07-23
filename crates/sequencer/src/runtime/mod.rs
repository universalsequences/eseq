//! Lisp-authorable sequencer extension runtimes.
//!
//! Each module here is the engine half of an eseqlisp `def-*` form: a unit the
//! scheduler ticks (or folds) each block on its own timebase. The engines are
//! deliberately lisp-agnostic — driven by plain Rust closures in tests and wired
//! to the scheduler-side lisp VM by `lisp_host`/`scheduler`:
//!
//! - [`process`] — `def-process` step-transform chains (pure fold semantics)
//! - [`accumulator`] — `def-accumulator` step accumulators
//! - [`generator`] — `def-generator` self-clocked emitters
//! - [`graph`] — graph-mode `def-sequencer` gather/scatter node fields
//!
//! The legacy builtin neural machine (`crate::neural`) is intentionally NOT in
//! this folder: it is not lisp-authorable (graph-mode `def-sequencer` is its
//! successor) and lives at the crate root on its own deprecation timeline.

pub mod accumulator;
pub mod generator;
pub mod graph;
pub mod process;
