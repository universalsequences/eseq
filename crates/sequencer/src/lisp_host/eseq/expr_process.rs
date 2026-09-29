/*!
Expr processes (docs/expr-process-spec.md §2, §2.1, §3, §10): compile the body
typed on an `expr` card into a hidden `expr#<hash>` process class.

The compile step parses the body, derives its inlets from the free symbols
in argument position, rejects what a process body cannot run, and hands a
`def-process` argument list to the same `parse_process_def` every builtin
class goes through. The result is an ordinary published process definition,
so inlet storage, wires, fan-out, serialization and the scheduler's run path
all work unchanged. The compiled definitions are held on `SequencerState`
(`register_expr_process_def`) and merged into every published authoring
read; slots keep the SOURCE, and `sync_expr_process_classes` recompiles every
stored body after a project load.

`$` context variables (§4, eseq-waa9.12) are rewritten at compile time:
the payload / transport ones to `(__expr-ctx <index>)` (one native match per
read), `$n` and `$prev` to hidden `:state` cells of the class that the run
body advances / stores at its top level (and zeroes on a node's first fire
after a reset). Direct writes (§6: `veto!`, `delay!`, `xpose!`, `vel!`,
`dur!`, `reset!`) lower to the existing verbs (`veto!`, `target-add!` /
`target-set!` on step-param ports, `graph-reset!`) and evaluate to nil; the
ports the used writes need are declared on the class.

State (§5, eseq-waa9.13): top-level `(state name init)` forms and the
stateful helpers (`prev delta integ sh slew every count`, one hidden cell
per call site) become extra `:state` cells that are NOT run-lambda
parameters: reads, `set!`s and helper updates go through the
`(__expr-cell op :key …)` native, which works on the invocation's state map
in place, so a write reaches the cell at any depth. They sit beside the
hidden `$n` / `$prev` cells, which user code can never name.
*/

use super::super::*;
use eseqlisp::parser::{Expr, ExprKind, SpannedASTParser, Token};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// The internal native every compiled body ends in: sends the body's value
/// on `out` (mappable) and `wire` (connectable).
pub(in crate::lisp_host) const EXPR_SEND_NATIVE: &str = "__expr-send!";

/// Deepest list nesting an expr body may have. The eseqlisp parser and
/// compiler both recurse per nesting level (eseq-4tl.1), so deeper source is
/// rejected on the flat token stream before either sees it (spec §10).
pub const EXPR_MAX_NESTING: usize = 32;
/// Most tokens one body may have: a card body is a line or a few, not a file.
pub const EXPR_MAX_TOKENS: usize = 4096;
/// Declared range of every derived inlet. Parsed symbols carry no range
/// (spec §3.1); the declared range only bounds pickers that read `:min` /
/// `:max`, it never clamps a value a cable writes.
pub const EXPR_INLET_MIN: f64 = -1.0e6;
pub const EXPR_INLET_MAX: f64 = 1.0e6;

/// The internal native `$` payload / transport reads lower to.
pub(in crate::lisp_host) const EXPR_CONTEXT_NATIVE: &str = "__expr-ctx";
/// Beats per bar for `$phase` (4/4, like `(bars n)`).
pub const EXPR_BAR_BEATS: f64 = 4.0;

/// `$` context variables a body may read (spec §4), with one-line docs for
/// the edit buffer's completion. A `$` name not listed here is a compile
/// error.
pub const EXPR_CONTEXT_VAR_DOCS: &[(&str, &str)] = &[
    ("$n", "fires of this node since its last reset (steps of this track since play), 0-based"),
    ("$prev", "this card's last wire output (0 before the first; a reset clears it)"),
    ("$note", "payload note, semitones, after earlier slots' writes"),
    ("$vel", "payload velocity 0..1, after earlier slots' writes"),
    ("$dur", "payload duration (beats on a node), after earlier slots' writes"),
    ("$delay", "propagation delay change so far, steps (nodes; 0 on tracks)"),
    ("$beat", "transport position, beats (now-beats)"),
    ("$phase", "position in the current 4/4 bar, 0..1"),
    ("$reset", "1 on a node's first fire after a reset, else 0"),
];

/// The `$` names, in [`EXPR_CONTEXT_VAR_DOCS`] order.
pub const EXPR_CONTEXT_VARS: &[&str] =
    &["$n", "$prev", "$note", "$vel", "$dur", "$delay", "$beat", "$phase", "$reset"];

/// Context variables read through `(__expr-ctx <index>)`; the index is the
/// position here and must match the native in `process_natives.rs`.
const EXPR_NATIVE_CONTEXT_VARS: &[&str] =
    &["$note", "$vel", "$dur", "$delay", "$beat", "$phase", "$reset"];
/// `$n` is a hidden state cell: -1 until the first fire, which advances it
/// to 0 (so the body reads 0-based fire counts).
const EXPR_STATE_N: &str = "$n";
/// `$prev` is a hidden state cell holding the last value sent.
const EXPR_STATE_PREV: &str = "$prev";

/// The internal native user state and helper cells go through (spec §5,
/// eseq-waa9.13): `(__expr-cell op :key args…)` reads or updates the
/// invocation's state map in place, so a `set!` on a state name reaches the
/// cell at any depth (inside `let`, `if`, `do`, a lambda), unlike a
/// def-process state cell bound as a run-lambda parameter.
pub(in crate::lisp_host) const EXPR_CELL_NATIVE: &str = "__expr-cell";

/// `__expr-cell` op codes; must match the native in `process_natives.rs`.
pub(in crate::lisp_host) mod expr_cell_op {
    pub const GET: u8 = 0;
    pub const SET: u8 = 1;
    pub const PREV: u8 = 2;
    pub const DELTA: u8 = 3;
    pub const INTEG: u8 = 4;
    pub const SH: u8 = 5;
    pub const SLEW: u8 = 6;
    pub const EVERY: u8 = 7;
    pub const COUNT: u8 = 8;
}

/// One stateful helper (spec §5.1): each call site owns a hidden cell.
struct ExprHelper {
    name: &'static str,
    args: usize,
    op: u8,
    signature: &'static str,
    doc: &'static str,
}

const EXPR_HELPERS: &[ExprHelper] = &[
    ExprHelper {
        name: "prev",
        args: 1,
        op: expr_cell_op::PREV,
        signature: "(prev x)",
        doc: "x from this call's previous fire (0 on the first)",
    },
    ExprHelper {
        name: "delta",
        args: 1,
        op: expr_cell_op::DELTA,
        signature: "(delta x)",
        doc: "x minus its value on the previous fire",
    },
    ExprHelper {
        name: "integ",
        args: 1,
        op: expr_cell_op::INTEG,
        signature: "(integ x)",
        doc: "running sum of x",
    },
    ExprHelper {
        name: "sh",
        args: 2,
        op: expr_cell_op::SH,
        signature: "(sh gate x)",
        doc: "sample x when gate > 0.5, else hold (0 until the first sample)",
    },
    ExprHelper {
        name: "slew",
        args: 2,
        op: expr_cell_op::SLEW,
        signature: "(slew x amt)",
        doc: "one-pole toward x: y += amt * (x - y), amt 0..1, y starts at 0",
    },
    ExprHelper {
        name: "every",
        args: 2,
        op: expr_cell_op::EVERY,
        signature: "(every k x)",
        doc: "x on the first fire and every k-th after it, else nil (sends nothing)",
    },
    ExprHelper {
        name: "count",
        args: 1,
        op: expr_cell_op::COUNT,
        signature: "(count k)",
        doc: "fire counter 0, 1, … k-1, 0, …",
    },
];

fn expr_helper(name: &str) -> Option<&'static ExprHelper> {
    EXPR_HELPERS.iter().find(|helper| helper.name == name)
}

/// The internal native the pure shaping helpers lower to (spec §6.1,
/// eseq-waa9.15): `(__expr-fn op args…)`, one native match per call.
pub(in crate::lisp_host) const EXPR_FN_NATIVE: &str = "__expr-fn";
/// The internal native `(choose a b …)` lowers to: picks one argument with
/// the per-fire process RNG (the stream `rand` draws from).
pub(in crate::lisp_host) const EXPR_CHOOSE_NATIVE: &str = "__expr-choose";

/// `__expr-fn` op codes; the index into [`EXPR_PURE_HELPERS`] order is not
/// used, the `op` field is.
pub(in crate::lisp_host) mod expr_fn_op {
    pub const QUANT: u8 = 0;
    pub const FOLD: u8 = 1;
    pub const WRAP: u8 = 2;
    pub const SCALE: u8 = 3;
    pub const CLIP: u8 = 4;
    pub const EUCLID: u8 = 5;
    pub const SINE: u8 = 6;
    pub const TRI: u8 = 7;
    pub const SAW: u8 = 8;
    pub const SQR: u8 = 9;
    pub const UNIPOLAR: u8 = 10;
    pub const BIPOLAR: u8 = 11;
}

/// One pure shaping helper (spec §6.1): no state, lowered to `__expr-fn`
/// (or `__expr-choose`) inside expr bodies only. None of these names is
/// defined as a scheduler global by this slice; `wrap` and `clip` already
/// are (same semantics), and win over the globals in function position here
/// the way the stateful helpers do.
struct ExprPureHelper {
    name: &'static str,
    min_args: usize,
    /// `usize::MAX` for variadic.
    max_args: usize,
    /// `None` for `choose`.
    op: Option<u8>,
    signature: &'static str,
    doc: &'static str,
}

const EXPR_PURE_HELPERS: &[ExprPureHelper] = &[
    ExprPureHelper {
        name: "quant",
        min_args: 2,
        max_args: 2,
        op: Some(expr_fn_op::QUANT),
        signature: "(quant x step)",
        doc: "x rounded to the nearest multiple of step (halves away from 0; step 0 = x)",
    },
    ExprPureHelper {
        name: "fold",
        min_args: 3,
        max_args: 3,
        op: Some(expr_fn_op::FOLD),
        signature: "(fold x lo hi)",
        doc: "x folded (ping-pong) into lo..hi",
    },
    ExprPureHelper {
        name: "wrap",
        min_args: 3,
        max_args: 3,
        op: Some(expr_fn_op::WRAP),
        signature: "(wrap x lo hi)",
        doc: "x wrapped into lo..hi (hi itself wraps to lo)",
    },
    ExprPureHelper {
        name: "scale",
        min_args: 5,
        max_args: 5,
        op: Some(expr_fn_op::SCALE),
        signature: "(scale x in-lo in-hi out-lo out-hi)",
        doc: "linear remap of x from in-lo..in-hi to out-lo..out-hi (not clamped)",
    },
    ExprPureHelper {
        name: "clip",
        min_args: 3,
        max_args: 3,
        op: Some(expr_fn_op::CLIP),
        signature: "(clip x lo hi)",
        doc: "x clamped to lo..hi",
    },
    ExprPureHelper {
        name: "euclid",
        min_args: 3,
        max_args: 3,
        op: Some(expr_fn_op::EUCLID),
        signature: "(euclid k n i)",
        doc: "1 when step i of a k-hit n-step euclidean rhythm is a hit, else 0 (i wraps mod n)",
    },
    ExprPureHelper {
        name: "choose",
        min_args: 1,
        max_args: usize::MAX,
        op: None,
        signature: "(choose a b …)",
        doc: "one of the arguments, uniformly, from the per-fire random stream",
    },
    ExprPureHelper {
        name: "sine",
        min_args: 1,
        max_args: 1,
        op: Some(expr_fn_op::SINE),
        signature: "(sine p)",
        doc: "sine of a 0..1 phase, as 0..1 (0.5 at p = 0)",
    },
    ExprPureHelper {
        name: "tri",
        min_args: 1,
        max_args: 1,
        op: Some(expr_fn_op::TRI),
        signature: "(tri p)",
        doc: "triangle of a 0..1 phase, 0..1 (0 at p = 0, 1 at p = 0.5)",
    },
    ExprPureHelper {
        name: "saw",
        min_args: 1,
        max_args: 1,
        op: Some(expr_fn_op::SAW),
        signature: "(saw p)",
        doc: "ramp of a phase: p wrapped into 0..1",
    },
    ExprPureHelper {
        name: "sqr",
        min_args: 1,
        max_args: 2,
        op: Some(expr_fn_op::SQR),
        signature: "(sqr p [duty])",
        doc: "pulse of a 0..1 phase: 1 while the wrapped phase < duty (0.5), else 0",
    },
    ExprPureHelper {
        name: "unipolar",
        min_args: 1,
        max_args: 1,
        op: Some(expr_fn_op::UNIPOLAR),
        signature: "(unipolar x)",
        doc: "-1..1 to 0..1",
    },
    ExprPureHelper {
        name: "bipolar",
        min_args: 1,
        max_args: 1,
        op: Some(expr_fn_op::BIPOLAR),
        signature: "(bipolar x)",
        doc: "0..1 to -1..1",
    },
];

fn expr_pure_helper(name: &str) -> Option<&'static ExprPureHelper> {
    EXPR_PURE_HELPERS.iter().find(|helper| helper.name == name)
}

/// `(name signature doc)` of every pure shaping helper, for completion.
pub fn expr_pure_helper_docs() -> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
    EXPR_PURE_HELPERS.iter().map(|helper| (helper.name, helper.signature, helper.doc))
}

/// Numeric constants an expr body reads as literals (never inlets, never
/// bindable).
pub const EXPR_CONSTANTS: &[(&str, f64, &str)] = &[
    ("pi", std::f64::consts::PI, "3.14159…"),
    ("tau", std::f64::consts::TAU, "2π = 6.28318…, one cycle in radians"),
];

fn expr_constant(name: &str) -> Option<f64> {
    EXPR_CONSTANTS.iter().find(|(constant, _, _)| *constant == name).map(|(_, value, _)| *value)
}

/// The threading forms' completion entries.
pub const EXPR_THREADING_DOCS: &[(&str, &str, &str)] = &[
    ("->", "(-> x f (g a) …)", "thread x through the forms as their first argument: (-> x (* 2) sin) = (sin (* x 2))"),
    ("->>", "(->> x f (g a) …)", "thread x through the forms as their last argument"),
];

/// Evaluate a pure shaping helper (`__expr-fn`). NaN in, NaN out; a
/// degenerate range is made well-defined (swapped bounds, an empty range
/// gives lo) so a picker passing through zero width never sends NaN.
pub fn expr_pure_fn(op: u8, args: &[f64]) -> Result<f64, String> {
    use expr_fn_op as f;
    let arg = |index: usize| args.get(index).copied();
    let need = |index: usize| arg(index).ok_or_else(|| format!("__expr-fn {op}: missing argument {index}"));
    let ordered = |lo: f64, hi: f64| if lo <= hi { (lo, hi) } else { (hi, lo) };
    // -0.0 reads as 0 in a picker; keep results on +0.
    let tidy = |value: f64| if value == 0.0 { 0.0 } else { value };
    // `rem_euclid` can round a tiny negative up to the modulus itself
    // (`(-1e-17).rem_euclid(1.0) == 1.0`); keep the half-open contract.
    let modulo = |value: f64, modulus: f64| {
        let r = value.rem_euclid(modulus);
        if r >= modulus { 0.0 } else { r }
    };
    // NaN propagates from any argument (spec §6.1), which also keeps a NaN
    // bound away from `f64::clamp` (it panics on a NaN min/max).
    if op <= f::BIPOLAR && args.iter().any(|value| value.is_nan()) {
        return Ok(f64::NAN);
    }
    let value = match op {
        f::QUANT => {
            let (x, step) = (need(0)?, need(1)?.abs());
            if step == 0.0 || !step.is_finite() {
                x
            } else {
                (x / step).round() * step
            }
        }
        f::FOLD => {
            let (x, (lo, hi)) = (need(0)?, ordered(need(1)?, need(2)?));
            let span = hi - lo;
            if span == 0.0 {
                lo
            } else {
                let phase = (x - lo).rem_euclid(span * 2.0);
                if phase <= span { lo + phase } else { hi - (phase - span) }
            }
        }
        f::WRAP => {
            let (x, (lo, hi)) = (need(0)?, ordered(need(1)?, need(2)?));
            let span = hi - lo;
            if span == 0.0 { lo } else { lo + modulo(x - lo, span) }
        }
        f::SCALE => {
            let (x, in_lo, in_hi, out_lo, out_hi) = (need(0)?, need(1)?, need(2)?, need(3)?, need(4)?);
            if in_hi == in_lo {
                out_lo
            } else {
                out_lo + (out_hi - out_lo) * (x - in_lo) / (in_hi - in_lo)
            }
        }
        f::CLIP => {
            let (x, (lo, hi)) = (need(0)?, ordered(need(1)?, need(2)?));
            if x.is_nan() { x } else { x.clamp(lo, hi) }
        }
        f::EUCLID => {
            let (k, n, i) = (need(0)?, need(1)?, need(2)?);
            if !(k.is_finite() && n.is_finite() && i.is_finite()) || n < 1.0 {
                0.0
            } else {
                let n = n.floor();
                let k = k.floor().clamp(0.0, n);
                let i = i.floor().rem_euclid(n);
                // Bresenham / Bjorklund-equivalent: step i is a hit when the
                // running k/n accumulator wraps on it (hits land on step 0).
                if (i * k).rem_euclid(n) < k { 1.0 } else { 0.0 }
            }
        }
        f::SINE => 0.5 + 0.5 * (std::f64::consts::TAU * need(0)?).sin(),
        f::TRI => 1.0 - (2.0 * modulo(need(0)?, 1.0) - 1.0).abs(),
        f::SAW => modulo(need(0)?, 1.0),
        f::SQR => {
            let duty = arg(1).unwrap_or(0.5);
            if modulo(need(0)?, 1.0) < duty { 1.0 } else { 0.0 }
        }
        f::UNIPOLAR => 0.5 + 0.5 * need(0)?,
        f::BIPOLAR => 2.0 * need(0)? - 1.0,
        other => return Err(format!("__expr-fn: unknown op {other}")),
    };
    Ok(tidy(value))
}

/// Expand `->` / `->>` on the AST (spec §6.1) with the compiler's own
/// semantics — a bare symbol stage `f` is `(f acc)`, a list stage gets acc
/// inserted first / last, and the threaded form is expanded again after the
/// insertion (so a stage that is itself `(-> …)` threads exactly as the VM
/// would). Quoted data is left alone. Runs before analysis so helpers
/// written through `->` get their cells and arity checks. Expansion can
/// deepen nesting (one level per stage), so the expanded depth is held to
/// the same [`EXPR_MAX_NESTING`] limit.
fn expand_threading(expr: &Expr, depth: usize) -> Result<Expr, ExprCompileError> {
    let ExprKind::List(items) = &expr.kind else {
        return Ok(expr.clone());
    };
    if depth >= EXPR_MAX_NESTING {
        return Err(ExprCompileError::at(
            format!("expr body nests deeper than {EXPR_MAX_NESTING} levels once -> is expanded"),
            expr,
        ));
    }
    let head = match items.first().map(|head| &head.kind) {
        Some(ExprKind::Symbol(head)) => head.as_str(),
        _ => "",
    };
    match head {
        "quote" => Ok(expr.clone()),
        "->" | "->>" => {
            let Some(initial) = items.get(1) else {
                return Err(ExprCompileError::at(format!("{head} needs a value to thread"), expr));
            };
            let last = head == "->>";
            let mut acc = initial.clone();
            for stage in &items[2..] {
                acc = match &stage.kind {
                    ExprKind::Symbol(_) => Expr::new(ExprKind::List(vec![stage.clone(), acc]), stage.origin.clone()),
                    ExprKind::List(parts) if !parts.is_empty() => {
                        let mut parts = parts.clone();
                        let at = if last { parts.len() } else { 1 };
                        parts.insert(at, acc);
                        Expr::new(ExprKind::List(parts), stage.origin.clone())
                    }
                    _ => {
                        return Err(ExprCompileError::at(
                            format!("a {head} stage must be a function name or a call"),
                            stage,
                        ))
                    }
                };
            }
            // The threaded form sits where the `->` form did.
            expand_threading(&acc, depth)
        }
        _ => Ok(Expr::new(
            ExprKind::List(
                items
                    .iter()
                    .map(|item| expand_threading(item, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            expr.origin.clone(),
        )),
    }
}

fn expand_threading_forms(forms: &[Expr]) -> Result<Vec<Expr>, ExprCompileError> {
    forms.iter().map(|form| expand_threading(form, 0)).collect()
}

/// The `state` form's completion entry.
pub const EXPR_STATE_FORM_DOC: (&str, &str, &str) = (
    "state",
    "(state name init)",
    "declare a state cell (top level of the body; init is a number literal); set! reaches it anywhere; a reset restores init",
);

/// `(name signature doc)` of every stateful helper, for completion.
pub fn expr_helper_docs() -> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
    EXPR_HELPERS.iter().map(|helper| (helper.name, helper.signature, helper.doc))
}

/// One direct write (spec §6).
struct ExprWriteVerb {
    name: &'static str,
    min_args: usize,
    max_args: usize,
    /// `(port step-param op)` for writes that go through a target port;
    /// `None` for the command verbs (`veto!`, `reset!`).
    port: Option<(&'static str, &'static str, &'static str)>,
    signature: &'static str,
    doc: &'static str,
}

/// The direct writes, in the order their ports are declared on a class.
const EXPR_WRITE_VERBS: &[ExprWriteVerb] = &[
    ExprWriteVerb {
        name: "veto!",
        min_args: 0,
        max_args: 0,
        port: None,
        signature: "(veto!)",
        doc: "mute this emission; the fire still scatters",
    },
    ExprWriteVerb {
        name: "delay!",
        min_args: 1,
        max_args: 1,
        port: Some(("delay!", "delay", "target-add!")),
        signature: "(delay! steps)",
        doc: "add steps to this fire's propagation delay (nodes)",
    },
    ExprWriteVerb {
        name: "xpose!",
        min_args: 1,
        max_args: 1,
        port: Some(("xpose!", "transpose", "target-add!")),
        signature: "(xpose! semitones)",
        doc: "add semitones to the payload note",
    },
    ExprWriteVerb {
        name: "vel!",
        min_args: 1,
        max_args: 1,
        port: Some(("vel!", "velocity", "target-set!")),
        signature: "(vel! v)",
        doc: "set the payload velocity, 0..1",
    },
    ExprWriteVerb {
        name: "dur!",
        min_args: 1,
        max_args: 1,
        port: Some(("dur!", "duration", "target-set!")),
        signature: "(dur! beats)",
        doc: "set the payload duration (beats on a node)",
    },
    ExprWriteVerb {
        name: "reset!",
        min_args: 0,
        max_args: 1,
        port: None,
        signature: "(reset! [group])",
        doc: "reset this node's graph after this fire: :all/0 or a group (1 = A, or a letter)",
    },
];

/// `(signature doc)` of every direct write, for the edit buffer's completion.
pub fn expr_write_verb_docs() -> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
    EXPR_WRITE_VERBS.iter().map(|verb| (verb.name, verb.signature, verb.doc))
}

/// `vel!` and `dur!` with two arguments are the ratchet-shape event
/// mutators `(vel! event v)`; only the one-argument form is a direct write.
fn write_verb(name: &str, argc: usize) -> Option<&'static ExprWriteVerb> {
    let verb = EXPR_WRITE_VERBS.iter().find(|verb| verb.name == name)?;
    if verb.port.is_some() && argc == 2 && matches!(name, "vel!" | "dur!") {
        return None;
    }
    Some(verb)
}

/// Names the compiled run body calls itself: an inlet named like one would
/// shadow it inside the body, so none of them can be an inlet.
const EXPR_RESERVED_NAMES: &[&str] = &[
    "target-add!",
    "target-set!",
    "graph-reset!",
    "veto!",
    "reset-fired?",
];

/// Reserved names and the direct writes: never an inlet, never a local.
fn is_reserved_name(name: &str) -> bool {
    name.starts_with("__")
        || EXPR_RESERVED_NAMES.contains(&name)
        || EXPR_WRITE_VERBS.iter().any(|verb| verb.name == name)
}

/// Special forms an expr body may use in function position. (`fn` is not
/// one: eseqlisp has no `fn` lambda, `((fn (a) a) 1)` fails at run time.)
const EXPR_SPECIAL_FORMS: &[&str] = &["if", "do", "let", "lambda", "and", "or", "quote", "set!"];
/// Heads the eseqlisp compiler lowers to opcodes; valid in function position
/// whether or not the VM also defines them as globals.
const EXPR_OPCODE_HEADS: &[&str] = &[
    "+", "-", "*", "/", "<", "<=", "=", ">", ">=", "list", "max", "min", "nth", "len",
];
/// Literal symbols.
const EXPR_LITERAL_SYMBOLS: &[&str] = &["true", "false", "nil"];

/// A compile failure, with the byte span in the body when the error has one.
#[derive(Clone, Debug, PartialEq)]
pub struct ExprCompileError {
    pub message: String,
    pub span: Option<(usize, usize)>,
}

impl ExprCompileError {
    pub(in crate::lisp_host) fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), span: None }
    }

    fn at(message: impl Into<String>, expr: &Expr) -> Self {
        let span = &expr.origin.primary_span;
        Self { message: message.into(), span: Some((span.start_byte, span.end_byte)) }
    }
}

impl std::fmt::Display for ExprCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// What the analyzer found in a body.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExprAnalysis {
    /// Derived inlets, in order of first appearance.
    pub inlets: Vec<String>,
    /// `$` names the body reads, in order of first appearance.
    pub context_vars: Vec<String>,
    /// Direct writes the body uses (spec §6), in order of first appearance.
    pub writes: Vec<String>,
    /// Inlets whose name is also a scheduler global function or macro: the
    /// inlet shadows the global inside this body (spec §3, as shipped .12).
    pub shadowing: Vec<String>,
    /// `(state name init)` cells, in declaration order (spec §5).
    pub state: Vec<(String, f64)>,
    /// Stateful helper call sites (spec §5.1), in source (pre-)order; each
    /// owns one hidden cell.
    pub helpers: Vec<String>,
}

/// A body compiled to a class.
#[derive(Clone, Debug)]
pub struct CompiledExpr {
    /// `expr` for an empty body, else `expr#<hash>`.
    pub class_name: String,
    pub inlets: Vec<String>,
    /// The hidden class; `None` for an empty body (the plain `expr` class).
    pub def: Option<crate::process::PublishedProcessDef>,
}

/// Names the scheduler VM resolves in a process body: its globals (natives
/// and the process / MIDI-fx library definitions) and its macros.
struct SchedulerNames {
    globals: HashSet<String>,
    /// The globals whose value can be called (natives, closures).
    callables: HashSet<String>,
    macros: HashSet<String>,
}

/// Built once per process from a throwaway scheduler scratch runtime loaded
/// the way the scheduler worker loads its own (`build_scheduler_scratch_runtime`),
/// so "visible to process bodies" means exactly what the scheduler VM has.
fn scheduler_names() -> &'static SchedulerNames {
    static NAMES: OnceLock<SchedulerNames> = OnceLock::new();
    NAMES.get_or_init(|| {
        let state = Arc::new(crate::sequencer::SequencerState::new(
            1,
            vec![crate::sequencer::default_empty_effect_chain()],
        ));
        let mut scratch = scheduler_scratch_runtime_with_fallbacks(state, 0, 0);
        if let Err(error) = scratch.eval(&load_midi_fx_library_source()) {
            eprintln!("[expr] MIDI FX library failed to load for name resolution: {error}");
        }
        if let Err(error) = scratch.eval(&load_process_library_source()) {
            eprintln!("[expr] process library failed to load for name resolution: {error}");
        }
        let runtime = &scratch.runtime;
        let globals: HashSet<String> = runtime
            .global_names()
            .iter()
            .filter(|name| runtime.has_global(name))
            .cloned()
            .collect();
        let callables = globals
            .iter()
            .filter(|name| {
                matches!(
                    runtime.global_value(name),
                    Some(
                        EValue::Closure(..)
                            | EValue::Function(_)
                            | EValue::NativeFunction(_)
                            | EValue::OverrideDispatcher(_)
                            | EValue::OverrideOriginal(_)
                            | EValue::HostHandle { .. }
                    )
                )
            })
            .cloned()
            .collect();
        SchedulerNames {
            globals,
            callables,
            macros: runtime.macros().keys().cloned().collect(),
        }
    })
}

fn is_callable_name(names: &SchedulerNames, name: &str) -> bool {
    EXPR_SPECIAL_FORMS.contains(&name)
        || EXPR_OPCODE_HEADS.contains(&name)
        || names.globals.contains(name)
        || names.macros.contains(name)
}

/// Parse a body into spanned forms, refusing deep or huge source before the
/// (recursive) AST parser runs.
fn parse_expr_forms(source: &str) -> Result<Vec<Expr>, ExprCompileError> {
    let tokens = eseqlisp::parser::Parser::new(source.to_string())
        .parse_spanned()
        .map_err(|error| ExprCompileError::new(format!("parse error: {error:?}")))?;
    if tokens.len() > EXPR_MAX_TOKENS {
        return Err(ExprCompileError::new(format!(
            "expr body is too long ({} tokens, limit {EXPR_MAX_TOKENS})",
            tokens.len()
        )));
    }
    let mut depth = 0usize;
    for token in &tokens {
        match token.token {
            Token::LeftParen => {
                depth += 1;
                if depth > EXPR_MAX_NESTING {
                    return Err(ExprCompileError {
                        message: format!("expr body nests deeper than {EXPR_MAX_NESTING} levels"),
                        span: Some((token.span.start_byte, token.span.end_byte)),
                    });
                }
            }
            Token::RightParen => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    SpannedASTParser::new(tokens)
        .parse()
        .map_err(|error| ExprCompileError::new(format!("parse error: {error:?}")))
}

/// Free-symbol analysis (spec §3). Recursion depth is bounded by the
/// nesting check in [`parse_expr_forms`].
struct Analyzer<'a> {
    names: &'a SchedulerNames,
    scopes: Vec<Vec<String>>,
    analysis: ExprAnalysis,
    /// Global names (not locals) the body calls, for the inlet/function
    /// clash check.
    function_uses: HashSet<String>,
    /// Byte span of each inlet's first appearance.
    inlet_spans: HashMap<String, (usize, usize)>,
    /// `(state …)` names: bound for the whole body (the outermost scope).
    state_names: Vec<String>,
    /// Lambdas the walk is inside (helpers are refused there).
    lambda_depth: usize,
    /// Every symbol the body calls in function position, local or not.
    called: HashSet<String>,
    /// Locals and state names that shadow a global function, macro or
    /// helper, with the span of their binding.
    local_shadowing: Vec<(String, (usize, usize))>,
}

/// Why a name cannot be bound by `let`, a lambda or `state`, if it cannot.
fn unbindable_name_error(name: &str, at: &Expr) -> Option<ExprCompileError> {
    if EXPR_SPECIAL_FORMS.contains(&name)
        || EXPR_OPCODE_HEADS.contains(&name)
        || EXPR_LITERAL_SYMBOLS.contains(&name)
        || is_reserved_name(name)
    {
        return Some(ExprCompileError::at(
            format!("local `{name}` shadows the built-in `{name}`; rename it"),
            at,
        ));
    }
    if name.starts_with('$') {
        return Some(ExprCompileError::at(
            format!("`{name}` is a context variable name and cannot be bound"),
            at,
        ));
    }
    if expr_constant(name).is_some() {
        return Some(ExprCompileError::at(
            format!("`{name}` is a constant and cannot be bound; rename the local"),
            at,
        ));
    }
    None
}

impl Analyzer<'_> {
    fn is_local(&self, name: &str) -> bool {
        self.scopes.iter().rev().any(|scope| scope.iter().any(|bound| bound == name))
    }

    /// A local or state name may shadow a global function, macro or helper
    /// (as an inlet may) unless the body also calls that name; remember it
    /// for that check.
    fn note_shadowing(&mut self, name: &str, at: &Expr) {
        if is_callable_name(self.names, name)
            || expr_helper(name).is_some()
            || expr_pure_helper(name).is_some()
            || name == "state"
        {
            let span = &at.origin.primary_span;
            self.local_shadowing.push((name.to_string(), (span.start_byte, span.end_byte)));
        }
    }

    fn bind(&mut self, name: &str, at: &Expr) -> Result<(), ExprCompileError> {
        if let Some(error) = unbindable_name_error(name, at) {
            return Err(error);
        }
        if self.state_names.iter().any(|state| state == name) {
            return Err(ExprCompileError::at(
                format!("`{name}` is a state cell of this body; a local cannot rebind it — rename the local"),
                at,
            ));
        }
        self.note_shadowing(name, at);
        self.scopes.last_mut().expect("scope").push(name.to_string());
        Ok(())
    }

    /// Collect the body's top-level `(state name [init])` forms before the
    /// walk, so a state name is bound for the whole body wherever its form
    /// sits. The init must be a number literal (hex / binary included).
    fn declare_state(&mut self, form: &Expr, items: &[Expr]) -> Result<(), ExprCompileError> {
        let usage = "state expects a name and a number: (state name init)";
        let (name_expr, init) = match items {
            [_, name] => (name, 0.0),
            [_, name, init] => match &init.kind {
                ExprKind::Number(value) if value.is_finite() => (name, *value),
                _ => {
                    return Err(ExprCompileError::at(
                        "a state's initial value must be a number literal",
                        init,
                    ))
                }
            },
            _ => return Err(ExprCompileError::at(usage, form)),
        };
        let ExprKind::Symbol(name) = &name_expr.kind else {
            return Err(ExprCompileError::at(usage, name_expr));
        };
        if name == "&" || name == "&rest" || name == "_" {
            return Err(ExprCompileError::at(format!("`{name}` cannot be a state name"), name_expr));
        }
        if let Some(error) = unbindable_name_error(name, name_expr) {
            return Err(error);
        }
        if self.state_names.iter().any(|seen| seen == name) {
            return Err(ExprCompileError::at(format!("state `{name}` is declared twice"), name_expr));
        }
        self.note_shadowing(name, name_expr);
        self.state_names.push(name.clone());
        self.scopes[0].push(name.clone());
        self.analysis.state.push((name.clone(), init));
        Ok(())
    }

    /// Bind one `let` name or lambda parameter. The VM binds plain symbols
    /// only: a destructuring pattern fails at run time, so it is refused here.
    fn bind_pattern(&mut self, pattern: &Expr) -> Result<(), ExprCompileError> {
        match &pattern.kind {
            ExprKind::Symbol(name) if name == "&rest" || name == "&" || name == "_" => Ok(()),
            ExprKind::Symbol(name) => self.bind(name, pattern),
            _ => Err(ExprCompileError::at("expected a name to bind", pattern)),
        }
    }

    fn argument_symbol(&mut self, name: &str, at: &Expr) -> Result<(), ExprCompileError> {
        if EXPR_LITERAL_SYMBOLS.contains(&name) || self.is_local(name) || expr_constant(name).is_some() {
            return Ok(());
        }
        // Inlets become lambda parameters of the run body, where these are
        // the rest markers.
        if name == "&" || name == "&rest" {
            return Err(ExprCompileError::at(format!("`{name}` cannot be an inlet name"), at));
        }
        if name.starts_with('$') {
            if !self.analysis.context_vars.iter().any(|seen| seen == name) {
                self.analysis.context_vars.push(name.to_string());
            }
            return Ok(());
        }
        // Special forms and opcode heads are lowered by the compiler whatever
        // is in scope, so they can never be shadowed; a global VALUE (not a
        // function) is read as is.
        if EXPR_SPECIAL_FORMS.contains(&name) || EXPR_OPCODE_HEADS.contains(&name) {
            return Ok(());
        }
        if self.names.globals.contains(name) && !self.names.callables.contains(name) {
            return Ok(());
        }
        if is_reserved_name(name) {
            return Err(ExprCompileError::at(
                format!("`{name}` is used by the expr card itself and cannot be an inlet; rename it"),
                at,
            ));
        }
        // Anything else is an inlet, including a name that is otherwise a
        // global function or macro (`vel`, `mod`, `in`): the inlet shadows
        // it for this body. `analyze_forms` refuses a body that also calls
        // that name.
        if !self.analysis.inlets.iter().any(|seen| seen == name) {
            self.analysis.inlets.push(name.to_string());
            let span = &at.origin.primary_span;
            self.inlet_spans.insert(name.to_string(), (span.start_byte, span.end_byte));
            if is_callable_name(self.names, name) {
                self.analysis.shadowing.push(name.to_string());
            }
        }
        Ok(())
    }

    fn walk(&mut self, expr: &Expr) -> Result<(), ExprCompileError> {
        match &expr.kind {
            ExprKind::Symbol(name) => self.argument_symbol(name, expr),
            ExprKind::Keyword(_)
            | ExprKind::String(_)
            | ExprKind::Number(_)
            | ExprKind::QuoteSymbol(_)
            | ExprKind::QuoteList(_) => Ok(()),
            ExprKind::Quasiquote(_) | ExprKind::Unquote(_) | ExprKind::UnquoteSplicing(_) => Err(
                ExprCompileError::at("quasiquote is not supported in an expr body", expr),
            ),
            ExprKind::List(items) => self.walk_list(expr, items),
        }
    }

    fn walk_all(&mut self, items: &[Expr]) -> Result<(), ExprCompileError> {
        items.iter().try_for_each(|item| self.walk(item))
    }

    fn walk_scoped_body(&mut self, body: &[Expr]) -> Result<(), ExprCompileError> {
        let result = self.walk_all(body);
        self.scopes.pop();
        result
    }

    fn walk_list(&mut self, expr: &Expr, items: &[Expr]) -> Result<(), ExprCompileError> {
        let Some(head) = items.first() else {
            return Ok(());
        };
        let args = &items[1..];
        let ExprKind::Symbol(name) = &head.kind else {
            // `((lambda (a) …) 1)`: the head is an expression.
            return self.walk_all(items);
        };
        self.called.insert(name.clone());
        match name.as_str() {
            "quote" => Ok(()),
            "state" if !self.is_local("state") => Err(ExprCompileError::at(
                "`state` declares a cell and belongs at the top level of the body",
                expr,
            )),
            "let" => {
                let Some(ExprKind::List(bindings)) = args.first().map(|b| &b.kind) else {
                    return Err(ExprCompileError::at("let expects a binding list", expr));
                };
                // Sequential (let*): each value sees the bindings before it.
                self.scopes.push(Vec::new());
                for binding in bindings {
                    let ExprKind::List(pair) = &binding.kind else {
                        self.scopes.pop();
                        return Err(ExprCompileError::at("let binding must be (name value)", binding));
                    };
                    if pair.len() != 2 {
                        self.scopes.pop();
                        return Err(ExprCompileError::at("let binding must be (name value)", binding));
                    }
                    if let Err(error) = self.walk(&pair[1]).and_then(|_| self.bind_pattern(&pair[0])) {
                        self.scopes.pop();
                        return Err(error);
                    }
                }
                self.walk_scoped_body(&args[1..])
            }
            "fn" if !self.is_local("fn") => Err(ExprCompileError::at("`fn` is not a lambda in eseqlisp; use `lambda`", head)),
            "lambda" => {
                let Some(params) = args.first() else {
                    return Err(ExprCompileError::at(format!("{name} expects a parameter list"), expr));
                };
                self.scopes.push(Vec::new());
                let bound = match &params.kind {
                    ExprKind::List(list) => list.iter().try_for_each(|param| self.bind_pattern(param)),
                    _ => Err(ExprCompileError::at(format!("{name} expects a parameter list"), params)),
                };
                if let Err(error) = bound {
                    self.scopes.pop();
                    return Err(error);
                }
                self.lambda_depth += 1;
                let result = self.walk_scoped_body(&args[1..]);
                self.lambda_depth -= 1;
                result
            }
            "set!" => {
                // `(set! name)` / `(set! name a b)` would reach the VM as a
                // malformed form (a state name lowers to a cell read there)
                // and fail every fire; refuse it at commit.
                if args.len() != 2 {
                    return Err(ExprCompileError::at("set! expects a name and a value", expr));
                }
                match args.first().map(|target| (&target.kind, target)) {
                    Some((ExprKind::Symbol(target), at)) if target.starts_with('$') => {
                        return Err(ExprCompileError::at(
                            format!("`{target}` is read-only context and cannot be set"),
                            at,
                        ));
                    }
                    Some((ExprKind::Symbol(target), _)) if self.is_local(target) => {}
                    Some((ExprKind::Symbol(target), at)) => {
                        return Err(ExprCompileError::at(
                            format!("set! on `{target}`: only names bound inside the body can be set"),
                            at,
                        ));
                    }
                    _ => return Err(ExprCompileError::at("set! expects a name and a value", expr)),
                }
                self.walk_all(&args[1..])
            }
            head_name if self.is_local(head_name) => self.walk_all(args),
            head_name if expr_helper(head_name).is_some() => {
                let helper = expr_helper(head_name).expect("helper");
                self.function_uses.insert(head_name.to_string());
                if args.len() != helper.args {
                    return Err(ExprCompileError::at(
                        format!(
                            "`{head_name}` takes {} argument{}: {}",
                            helper.args,
                            if helper.args == 1 { "" } else { "s" },
                            helper.signature
                        ),
                        expr,
                    ));
                }
                if self.lambda_depth > 0 {
                    return Err(ExprCompileError::at(
                        format!(
                            "`{head_name}` keeps one history per place it is written and cannot be used inside a lambda; move it out of the lambda"
                        ),
                        expr,
                    ));
                }
                self.analysis.helpers.push(head_name.to_string());
                self.walk_all(args)
            }
            head_name if expr_pure_helper(head_name).is_some() => {
                let helper = expr_pure_helper(head_name).expect("pure helper");
                self.function_uses.insert(head_name.to_string());
                if args.len() < helper.min_args || args.len() > helper.max_args {
                    let count = match (helper.min_args, helper.max_args) {
                        (lo, usize::MAX) => format!("at least {lo} argument{}", if lo == 1 { "" } else { "s" }),
                        (lo, hi) if lo == hi => format!("{lo} argument{}", if lo == 1 { "" } else { "s" }),
                        (lo, hi) => format!("{lo} to {hi} arguments"),
                    };
                    return Err(ExprCompileError::at(
                        format!("`{head_name}` takes {count}: {}", helper.signature),
                        expr,
                    ));
                }
                self.walk_all(args)
            }
            head_name if head_name.starts_with("def") && !EXPR_OPCODE_HEADS.contains(&head_name) => {
                Err(ExprCompileError::at(
                    format!("`{head_name}` defines a global and is not allowed in an expr body"),
                    head,
                ))
            }
            head_name if EXPR_WRITE_VERBS.iter().any(|verb| verb.name == head_name) => {
                self.function_uses.insert(head_name.to_string());
                match write_verb(head_name, args.len()) {
                    Some(verb) if args.len() < verb.min_args || args.len() > verb.max_args => {
                        Err(ExprCompileError::at(
                            format!("`{head_name}` takes {}: {}", match (verb.min_args, verb.max_args) {
                                (0, 0) => "no arguments".to_string(),
                                (lo, hi) if lo == hi => format!("{lo} argument{}", if lo == 1 { "" } else { "s" }),
                                (lo, hi) => format!("{lo} to {hi} arguments"),
                            }, verb.signature),
                            expr,
                        ))
                    }
                    Some(verb) => {
                        if !self.analysis.writes.iter().any(|seen| seen == verb.name) {
                            self.analysis.writes.push(verb.name.to_string());
                        }
                        self.walk_all(args)
                    }
                    // `(vel! event v)`: the ratchet-shape mutator.
                    None => self.walk_all(args),
                }
            }
            head_name if is_callable_name(self.names, head_name) => {
                self.function_uses.insert(head_name.to_string());
                self.walk_all(args)
            }
            head_name => Err(ExprCompileError::at(format!("unknown function `{head_name}`"), head)),
        }
    }
}

/// The items of a top-level `(state …)` form. (A body cannot bind `state`
/// before its top level runs, so the head is always the declaration here.)
fn state_form_items(form: &Expr) -> Option<&[Expr]> {
    match &form.kind {
        ExprKind::List(items)
            if matches!(items.first().map(|head| &head.kind), Some(ExprKind::Symbol(head)) if head == "state") =>
        {
            Some(items)
        }
        _ => None,
    }
}

fn analyze_forms(forms: &[Expr]) -> Result<ExprAnalysis, ExprCompileError> {
    let mut analyzer = Analyzer {
        names: scheduler_names(),
        scopes: vec![Vec::new()],
        analysis: ExprAnalysis::default(),
        function_uses: HashSet::new(),
        inlet_spans: HashMap::new(),
        state_names: Vec::new(),
        lambda_depth: 0,
        called: HashSet::new(),
        local_shadowing: Vec::new(),
    };
    for form in forms {
        if let Some(items) = state_form_items(form) {
            analyzer.declare_state(form, items)?;
        }
    }
    for form in forms {
        if state_form_items(form).is_none() {
            analyzer.walk(form)?;
        }
    }
    // A local or state name shadows a global function of the same name in
    // its scope; calling that name anywhere in the body is refused, like an
    // inlet used as a function below.
    if let Some((name, span)) = analyzer
        .local_shadowing
        .iter()
        .find(|(name, _)| analyzer.called.contains(name.as_str()))
    {
        return Err(ExprCompileError {
            message: format!(
                "`{name}` is bound in the body and also called as a function — rename the local"
            ),
            span: Some(*span),
        });
    }
    // An inlet shadows a global function of the same name for the whole
    // body, so the body cannot also call it.
    if let Some(name) = analyzer
        .analysis
        .inlets
        .iter()
        .find(|name| analyzer.function_uses.contains(name.as_str()))
    {
        return Err(ExprCompileError {
            message: format!(
                "`{name}` is used both as an inlet and as a function — rename the inlet"
            ),
            span: analyzer.inlet_spans.get(name).copied(),
        });
    }
    Ok(analyzer.analysis)
}

/// Derive a body's inlets and context reads without compiling it (spec §3).
pub fn analyze_expr_source(source: &str) -> Result<ExprAnalysis, ExprCompileError> {
    analyze_forms(&expand_threading_forms(&parse_expr_forms(source)?)?)
}

/// The body's forms re-printed, whitespace- and comment-insensitive.
fn normalized_forms(forms: &[Expr]) -> String {
    forms
        .iter()
        .map(|form| eseqlisp::parser::format_expression(&form.to_legacy()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn expr_class_name_for_normalized(normalized: &str) -> String {
    let hash = crate::process::stable_process_id(&format!("expr:{normalized}"));
    format!("{}{:012x}", crate::process::EXPR_PROCESS_CLASS_PREFIX, hash & 0xFFFF_FFFF_FFFF)
}

/// The class a body compiles to, from parsing alone (no name checks): the
/// plain `expr` class for an empty body, else `expr#<hash>`. `None` when the
/// body does not parse.
pub fn expr_class_name_for_source(source: &str) -> Option<String> {
    let forms = parse_expr_forms(source).ok()?;
    Some(if forms.is_empty() {
        crate::process::EXPR_PROCESS_CLASS.to_string()
    } else {
        expr_class_name_for_normalized(&normalized_forms(&forms))
    })
}

fn expr_to_value(expr: &Expr) -> Result<EValue, ExprCompileError> {
    Ok(match &expr.kind {
        ExprKind::Symbol(name) => EValue::Symbol(name.clone()),
        ExprKind::Keyword(name) => EValue::Keyword(name.clone()),
        ExprKind::String(text) => EValue::String(text.clone()),
        ExprKind::Number(number) => EValue::Number(*number),
        ExprKind::QuoteSymbol(name) => {
            process_list([EValue::Symbol("quote".to_string()), EValue::Symbol(name.clone())])
        }
        ExprKind::QuoteList(items) => process_list([
            EValue::Symbol("quote".to_string()),
            process_list(items.iter().map(expr_to_value).collect::<Result<Vec<_>, _>>()?),
        ]),
        ExprKind::List(items) => {
            process_list(items.iter().map(expr_to_value).collect::<Result<Vec<_>, _>>()?)
        }
        ExprKind::Quasiquote(_) | ExprKind::Unquote(_) | ExprKind::UnquoteSplicing(_) => {
            return Err(ExprCompileError::at("quasiquote is not supported in an expr body", expr));
        }
    })
}

/// [`expr_to_value`] plus the expr rewrites (spec §4, §5, §6):
///
/// - `$note` … `$reset` become `(__expr-ctx <index>)`; `$n` / `$prev` stay
///   symbols (the run body binds them to state cells);
/// - a direct write becomes its verb followed by nil, so a body that only
///   writes sends nothing;
/// - a state name reads `(__expr-cell 0 :name)` and `(set! name v)` writes
///   `(__expr-cell 1 :name v)`, at any depth;
/// - each stateful helper call site gets its own hidden cell
///   (`__h<n>-<helper>`, numbered in source pre-order) and becomes
///   `(__expr-cell <op> :__h<n>-<helper> args…)`.
///
/// Quoted data is left alone. `let` binding names and lambda parameter lists
/// are copied as is. No other scope tracking is needed: the analyzer refuses
/// a local named like a state cell, `$` name or write verb, and a body that
/// both binds a helper's name and calls it.
struct Lowerer<'a> {
    state: &'a [(String, f64)],
    /// `(cell name, helper)` per helper call site, in allocation order.
    helper_cells: Vec<(String, &'static str)>,
}

impl Lowerer<'_> {
    fn is_state(&self, name: &str) -> bool {
        self.state.iter().any(|(state, _)| state == name)
    }

    fn cell_call(op: u8, key: &str, args: Vec<EValue>) -> EValue {
        process_list(
            [
                EValue::Symbol(EXPR_CELL_NATIVE.to_string()),
                EValue::Number(op as f64),
                EValue::Keyword(key.to_string()),
            ]
            .into_iter()
            .chain(args),
        )
    }

    fn lower_all(&mut self, items: &[Expr]) -> Result<Vec<EValue>, ExprCompileError> {
        items.iter().map(|item| self.lower(item)).collect()
    }

    fn lower(&mut self, expr: &Expr) -> Result<EValue, ExprCompileError> {
        let symbol = |name: &str| EValue::Symbol(name.to_string());
        match &expr.kind {
            ExprKind::Symbol(name) => {
                if let Some(index) = EXPR_NATIVE_CONTEXT_VARS.iter().position(|var| var == name) {
                    return Ok(process_list([
                        symbol(EXPR_CONTEXT_NATIVE),
                        EValue::Number(index as f64),
                    ]));
                }
                if self.is_state(name) {
                    return Ok(Self::cell_call(expr_cell_op::GET, name, Vec::new()));
                }
                if let Some(value) = expr_constant(name) {
                    return Ok(EValue::Number(value));
                }
                expr_to_value(expr)
            }
            ExprKind::List(items) => {
                let head = match items.first().map(|head| &head.kind) {
                    Some(ExprKind::Symbol(head)) => Some(head.as_str()),
                    _ => None,
                };
                let args = items.get(1..).unwrap_or(&[]);
                match head {
                    Some("quote") => return expr_to_value(expr),
                    Some("let") => {
                        // (let ((name value) …) body…): lower the values and
                        // the body, never a binding name.
                        let mut out = vec![symbol("let")];
                        if let Some(bindings) = args.first() {
                            let ExprKind::List(pairs) = &bindings.kind else {
                                return expr_to_value(expr);
                            };
                            let mut lowered = Vec::with_capacity(pairs.len());
                            for pair in pairs {
                                match &pair.kind {
                                    ExprKind::List(parts) if parts.len() == 2 => {
                                        lowered.push(process_list([
                                            expr_to_value(&parts[0])?,
                                            self.lower(&parts[1])?,
                                        ]));
                                    }
                                    _ => lowered.push(expr_to_value(pair)?),
                                }
                            }
                            out.push(process_list(lowered));
                            out.extend(self.lower_all(&args[1..])?);
                        }
                        return Ok(process_list(out));
                    }
                    Some("lambda") => {
                        let mut out = vec![symbol("lambda")];
                        if let Some(params) = args.first() {
                            out.push(expr_to_value(params)?);
                            out.extend(self.lower_all(&args[1..])?);
                        }
                        return Ok(process_list(out));
                    }
                    Some("set!") => {
                        if let [target, value] = args {
                            if let ExprKind::Symbol(name) = &target.kind {
                                if self.is_state(name) {
                                    let value = self.lower(value)?;
                                    return Ok(Self::cell_call(expr_cell_op::SET, name, vec![value]));
                                }
                            }
                        }
                    }
                    _ => {}
                }
                if let Some(helper) = head.and_then(expr_helper) {
                    let cell = format!("__h{}-{}", self.helper_cells.len(), helper.name);
                    self.helper_cells.push((cell.clone(), helper.name));
                    let lowered = self.lower_all(args)?;
                    return Ok(Self::cell_call(helper.op, &cell, lowered));
                }
                if let Some(helper) = head.and_then(expr_pure_helper) {
                    let lowered = self.lower_all(args)?;
                    return Ok(match helper.op {
                        Some(op) => process_list(
                            [symbol(EXPR_FN_NATIVE), EValue::Number(op as f64)].into_iter().chain(lowered),
                        ),
                        None => process_list(std::iter::once(symbol(EXPR_CHOOSE_NATIVE)).chain(lowered)),
                    });
                }
                if let Some(verb) = head.and_then(|head| write_verb(head, args.len())) {
                    let call = match verb.port {
                        Some((port, _, op)) => {
                            let mut call = vec![symbol(op), EValue::Keyword(port.to_string())];
                            call.extend(self.lower_all(args)?);
                            call
                        }
                        None if verb.name == "reset!" => {
                            std::iter::once(symbol("graph-reset!")).chain(self.lower_all(args)?).collect()
                        }
                        None => std::iter::once(symbol(verb.name)).chain(self.lower_all(args)?).collect(),
                    };
                    return Ok(process_list([symbol("do"), process_list(call), EValue::Nil]));
                }
                Ok(process_list(self.lower_all(items)?))
            }
            _ => expr_to_value(expr),
        }
    }
}

/// The run body of the hidden class and the cells it keeps through
/// `__expr-cell` (user state, then helper cells, with their initial values).
///
/// The lowered body is sent by `__expr-send!`, with the `$n` / `$prev`
/// bookkeeping when the body reads them. Those two cells are parameters of
/// the run lambda (def-process state binding), so every `set!` on them sits
/// at the lambda body's top level, where it reaches the cell. The other
/// cells are not lambda parameters: they are read and written in place in
/// the invocation's state map. On a node's first fire after a reset they
/// all return to their initial values before the body runs.
fn expr_run_body(
    forms: &[Expr],
    analysis: &ExprAnalysis,
) -> Result<(EValue, Vec<(String, f64)>), ExprCompileError> {
    let symbol = |name: &str| EValue::Symbol(name.to_string());
    let mut lowerer = Lowerer { state: &analysis.state, helper_cells: Vec::new() };
    let body_forms: Vec<&Expr> =
        forms.iter().filter(|form| state_form_items(form).is_none()).collect();
    let body = match body_forms.as_slice() {
        [] => EValue::Nil,
        [single] => lowerer.lower(single)?,
        many => process_list(
            std::iter::once(Ok(symbol("do")))
                .chain(many.iter().map(|form| lowerer.lower(form)))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };
    debug_assert_eq!(
        lowerer.helper_cells.iter().map(|(_, helper)| *helper).collect::<Vec<_>>(),
        analysis.helpers,
        "analyzer and lowering agree on helper call sites"
    );
    let cells: Vec<(String, f64)> = analysis
        .state
        .iter()
        .cloned()
        .chain(lowerer.helper_cells.into_iter().map(|(cell, _)| (cell, 0.0)))
        .collect();
    let reads = |name: &str| analysis.context_vars.iter().any(|var| var == name);
    let reset_fired = || process_list([symbol("reset-fired?")]);
    let mut steps = Vec::new();
    if !cells.is_empty() {
        // (if (reset-fired?) (do (__expr-cell 1 :s init) …) nil)
        steps.push(process_list([
            symbol("if"),
            reset_fired(),
            process_list(std::iter::once(symbol("do")).chain(cells.iter().map(|(cell, init)| {
                Lowerer::cell_call(expr_cell_op::SET, cell, vec![EValue::Number(*init)])
            }))),
            EValue::Nil,
        ]));
    }
    if reads(EXPR_STATE_N) {
        // (set! $n (if (reset-fired?) 0 (+ $n 1)))
        steps.push(process_list([
            symbol("set!"),
            symbol(EXPR_STATE_N),
            process_list([
                symbol("if"),
                reset_fired(),
                EValue::Number(0.0),
                process_list([symbol("+"), symbol(EXPR_STATE_N), EValue::Number(1.0)]),
            ]),
        ]));
    }
    if reads(EXPR_STATE_PREV) {
        // (set! $prev (if (reset-fired?) 0 $prev))
        // (set! $prev (__expr-send! body $prev))
        steps.push(process_list([
            symbol("set!"),
            symbol(EXPR_STATE_PREV),
            process_list([
                symbol("if"),
                reset_fired(),
                EValue::Number(0.0),
                symbol(EXPR_STATE_PREV),
            ]),
        ]));
        steps.push(process_list([
            symbol("set!"),
            symbol(EXPR_STATE_PREV),
            process_list([symbol(EXPR_SEND_NATIVE), body, symbol(EXPR_STATE_PREV)]),
        ]));
    } else {
        steps.push(process_list([symbol(EXPR_SEND_NATIVE), body]));
    }
    let run = if steps.len() == 1 {
        steps.pop().expect("one step")
    } else {
        process_list(std::iter::once(symbol("do")).chain(steps))
    };
    Ok((run, cells))
}

/// Compile a body (spec §2 steps 1–2): parse, derive inlets, check every
/// function-position name against what the scheduler VM defines, and build
/// the hidden class through `parse_process_def`. Inlet symbols are bound by
/// def-process's standard inlet binding (each inlet is a lambda parameter
/// bound to `(in :name)`), which is the spec's "rewrite to `(in :name)`".
pub fn compile_expr_source(source: &str) -> Result<CompiledExpr, ExprCompileError> {
    let forms = parse_expr_forms(source)?;
    if forms.is_empty() {
        return Ok(CompiledExpr {
            class_name: crate::process::EXPR_PROCESS_CLASS.to_string(),
            inlets: Vec::new(),
            def: None,
        });
    }
    let class_name = expr_class_name_for_normalized(&normalized_forms(&forms));
    let (def, inlets) = expr_process_def(&class_name, source, &forms, None, &[])?;
    let published = crate::process::ProcessAuthoringSnapshot {
        defs: vec![def],
        ..Default::default()
    }
    .to_published()
    .map_err(ExprCompileError::new)?
    .defs
    .pop()
    .ok_or_else(|| ExprCompileError::new("expr class did not publish"))?;
    Ok(CompiledExpr { class_name, inlets, def: Some(published) })
}

/// Build the class `class_name` from a body's parsed `forms` — the one
/// lowering both an expr card's hidden `expr#<hash>` class and a promoted
/// `def-process … :expr "…"` class go through (spec §8), so the two run
/// identically. `doc` replaces the generated `expr: <body>` doc; each of
/// `overrides` replaces the derived inlet of the same name (range, default,
/// kind, lane, doc) and must name one. Returns the def (with `expr_source`
/// set) and the derived inlet names in order.
fn expr_process_def(
    class_name: &str,
    source: &str,
    forms: &[Expr],
    doc: Option<String>,
    overrides: &[crate::process::ProcessInletDef],
) -> Result<(crate::process::ProcessDef, Vec<String>), ExprCompileError> {
    // `->` / `->>` expand first, so helpers inside a threaded form get their
    // arity check and cells; the class hash stays on the authored forms.
    let expanded = expand_threading_forms(forms)?;
    let analysis = analyze_forms(&expanded)?;
    if let Some(unknown) = analysis
        .context_vars
        .iter()
        .find(|name| !EXPR_CONTEXT_VARS.contains(&name.as_str()))
    {
        return Err(ExprCompileError::new(format!("unknown context variable `{unknown}`")));
    }
    if let Some(extra) = overrides
        .iter()
        .find(|entry| !analysis.inlets.iter().any(|inlet| inlet == &entry.name))
    {
        return Err(ExprCompileError::new(format!(
            ":in declares `{}`, which the expr body does not use as an inlet",
            extra.name
        )));
    }
    let normalized = normalized_forms(forms);
    let keyword = |name: &str| EValue::Keyword(name.to_string());
    let symbol = |name: &str| EValue::Symbol(name.to_string());
    let inlet_decls = process_list(analysis.inlets.iter().map(|inlet| {
        process_list([
            symbol(inlet),
            symbol("float"),
            EValue::Number(EXPR_INLET_MIN),
            EValue::Number(EXPR_INLET_MAX),
            keyword("default"),
            EValue::Number(0.0),
            keyword("lane"),
            EValue::Bool(true),
        ])
    }));
    // `out` and `wire`, then one fixed step-param port per direct write the
    // body uses, in `EXPR_WRITE_VERBS` order.
    let targets = process_list(
        [
            process_list([symbol("out"), keyword("mappable")]),
            process_list([symbol("wire"), keyword("process-inlet")]),
        ]
        .into_iter()
        .chain(EXPR_WRITE_VERBS.iter().filter_map(|verb| {
            let (port, param, _) = verb.port?;
            analysis.writes.iter().any(|used| used == verb.name).then(|| {
                process_list([
                    symbol(port),
                    process_list([symbol("step-param"), keyword(param)]),
                ])
            })
        })),
    );
    let reads = |name: &str| analysis.context_vars.iter().any(|var| var == name);
    let state_cells = process_list(
        [(EXPR_STATE_N, -1.0), (EXPR_STATE_PREV, 0.0)]
            .into_iter()
            .filter(|(name, _)| reads(name))
            .map(|(name, initial)| process_list([symbol(name), EValue::Number(initial)])),
    );
    let mut args = vec![
        keyword("doc"),
        EValue::String(format!("expr: {normalized}")),
        keyword("in"),
        inlet_decls,
        keyword("targets"),
        targets,
    ];
    if reads(EXPR_STATE_N) || reads(EXPR_STATE_PREV) {
        args.extend([keyword("state"), state_cells]);
    }
    let (run, cells) = expr_run_body(&expanded, &analysis)?;
    args.extend([keyword("run"), run]);
    let mut def = parse_process_def(class_name, &args).map_err(ExprCompileError::new)?;
    // User state and helper cells are declared AFTER the run body is
    // wrapped: def-process would bind every `:state` name as a run-lambda
    // parameter and store the parameter back at the end, overwriting the
    // in-place `__expr-cell` writes. Declared here they still get their
    // initial values (and stop → play clears) from the runtime, which fills
    // missing cells from `ProcessDef.state`.
    def.state.extend(cells.into_iter().map(|(name, initial)| crate::process::ProcessStateDef {
        name,
        initial: EValue::Number(initial),
    }));
    // Overrides change what the inlet looks like (picker range, default,
    // doc), never its name or position: the run lambda binds inlets by name.
    for inlet in &mut def.inlets {
        if let Some(entry) = overrides.iter().find(|entry| entry.name == inlet.name) {
            *inlet = entry.clone();
        }
    }
    if let Some(doc) = doc {
        def.doc = Some(doc);
    }
    def.expr_source = Some(source.to_string());
    Ok((def, analysis.inlets))
}

/// The `:expr` option of a `def-process` argument list (after the name), if
/// present: `Ok(None)` without one, an error when its value is not a string.
pub(in crate::lisp_host) fn def_process_expr_option(args: &[EValue]) -> Result<Option<String>, String> {
    let mut idx = 0;
    while idx + 1 < args.len() {
        let is_expr = matches!(&args[idx], EValue::Keyword(key) | EValue::Symbol(key)
            if key.trim_start_matches(':').eq_ignore_ascii_case("expr"));
        if is_expr {
            return match &args[idx + 1] {
                EValue::String(source) => Ok(Some(source.clone())),
                other => Err(format!("def-process :expr expects the body as a string, got {other:?}")),
            };
        }
        idx += 2;
    }
    Ok(None)
}

/// `(def-process name :doc "…" :in (…) :expr "<body>")` (spec §8): a class
/// compiled from an expr body through the same pipeline as an expr card, so
/// a card promoted to My processes behaves exactly like the card. Only
/// `:doc` and `:in` (overrides of derived inlets: range, default, doc) may
/// accompany `:expr`; the body supplies inlets, state, targets and `:run`.
pub(in crate::lisp_host) fn expr_process_def_from_args(
    name: &str,
    source: &str,
    args: &[EValue],
) -> Result<crate::process::ProcessDef, String> {
    let mut doc = None;
    let mut overrides = Vec::new();
    let mut idx = 0;
    while idx < args.len() {
        let key = process_symbol_name(&args[idx])?.to_ascii_lowercase();
        let value = args
            .get(idx + 1)
            .ok_or_else(|| format!("def-process missing value for :{key}"))?;
        match key.as_str() {
            "expr" => {}
            "doc" => {
                if let EValue::String(text) = value {
                    doc = Some(text.clone());
                }
            }
            "in" => overrides = parse_process_inlets(value)?,
            other => {
                return Err(format!(
                    "def-process {name}: an :expr class takes only :doc and :in beside :expr (got :{other})"
                ))
            }
        }
        idx += 2;
    }
    let forms = parse_expr_forms(source).map_err(|error| format!("def-process {name} :expr: {error}"))?;
    if forms.is_empty() {
        return Err(format!("def-process {name}: the :expr body is empty"));
    }
    expr_process_def(name, source, &forms, doc, &overrides)
        .map(|(def, _)| def)
        .map_err(|error| format!("def-process {name} :expr: {error}"))
}

/// Compile a body and register its class on `state` (a no-op for an empty
/// body or an already-registered class).
pub fn compile_and_register_expr_source(
    state: &crate::sequencer::SequencerState,
    source: &str,
) -> Result<CompiledExpr, ExprCompileError> {
    let compiled = compile_expr_source(source)?;
    if let Some(def) = &compiled.def {
        state.register_expr_process_def(def.clone());
    }
    Ok(compiled)
}

/// Whether a slot is an expr card (the plain class or a compiled one).
pub fn is_expr_slot(slot: &crate::process::TrackProcessSlot) -> bool {
    slot.is_expr_card()
}

/// Result of rebinding one slot to a new body (spec §2 steps 3–4).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExprRebind {
    /// Inlets the old class had that the new one lacks; their values,
    /// lanes, and every wire / fan-out cable into them were dropped.
    pub removed: Vec<String>,
}

/// Rebind `slot_id` in `chain` to `compiled`, in place: same instance id,
/// same position, same outgoing bindings and fan-out. Inlets reconcile by
/// name: surviving names keep their values and incoming cables (retargeted
/// to the new class), new names start at their default (0), removed names
/// lose their values and incoming cables.
pub fn rebind_expr_slot(
    chain: &mut crate::process::TrackProcessChain,
    slot_id: crate::process::ProcessInstanceId,
    old_inlets: &[String],
    source: &str,
    compiled: &CompiledExpr,
) -> Result<ExprRebind, String> {
    rebind_slot_class(
        chain,
        slot_id,
        old_inlets,
        &compiled.class_name,
        &compiled.inlets,
        Some(source),
    )
}

/// Rebind `slot_id` in `chain` to `new_class` in place (same instance id,
/// position, outgoing bindings and fan-out), reconciling inlets by name as
/// [`rebind_expr_slot`] does. `expr_source` is the slot's new body: `Some`
/// keeps / makes it an expr card, `None` makes it an ordinary library card
/// (promote, spec §8). Runtime state is keyed by the slot instance and
/// reconciled by cell name, so it carries over when the cells keep their
/// names (a promote keeps every one).
pub fn rebind_slot_class(
    chain: &mut crate::process::TrackProcessChain,
    slot_id: crate::process::ProcessInstanceId,
    old_inlets: &[String],
    new_class: &str,
    new_inlets: &[String],
    expr_source: Option<&str>,
) -> Result<ExprRebind, String> {
    use crate::process::ParamTarget;
    let position = chain
        .slots
        .iter()
        .position(|slot| slot.instance_id == slot_id)
        .ok_or_else(|| format!("no slot {}", slot_id.0))?;
    let keep: BTreeSet<&str> = new_inlets.iter().map(String::as_str).collect();
    let mut removed: Vec<String> = old_inlets
        .iter()
        .filter(|name| !keep.contains(name.as_str()))
        .cloned()
        .collect();
    {
        let slot = &mut chain.slots[position];
        for name in slot.inlets.keys().chain(slot.lanes.keys()) {
            if !keep.contains(name.as_str()) && !removed.contains(name) {
                removed.push(name.clone());
            }
        }
        slot.inlets.retain(|name, _| keep.contains(name.as_str()));
        slot.lanes.retain(|name, _| keep.contains(name.as_str()));
        slot.class_name = new_class.to_string();
        slot.expr_source = expr_source.map(str::to_string);
    }
    // Incoming wires and fan-out cables are keyed by (class, inlet,
    // instance id): retarget the survivors to the new class, drop the rest.
    let new_class = new_class.to_string();
    let retarget = |target: &mut ParamTarget| -> bool {
        match target {
            ParamTarget::ProcessInlet { process, inlet, instance_id: Some(id) } if *id == slot_id => {
                if keep.contains(inlet.as_str()) {
                    *process = new_class.clone();
                    true
                } else {
                    false
                }
            }
            _ => true,
        }
    };
    for slot in &mut chain.slots {
        slot.bindings.retain(|_, target| match target {
            Some(target) => retarget(target),
            None => true,
        });
        for entries in slot.fanout.values_mut() {
            entries.retain_mut(|entry| retarget(&mut entry.target));
        }
        slot.fanout.retain(|_, entries| !entries.is_empty());
    }
    Ok(ExprRebind { removed })
}

/// Every expr body stored in the scene bank's graph overrides (every scene
/// and every rack clip).
pub fn expr_sources_in_scenes(scenes: &crate::sequencer::ProjectScenes) -> BTreeSet<String> {
    let mut sources = BTreeSet::new();
    let graphs = scenes
        .scenes
        .iter()
        .flat_map(|scene| scene.graph_overrides.iter())
        .chain(
            scenes
                .rack_banks
                .iter()
                .flat_map(|bank| bank.clips.iter())
                .flat_map(|clip| clip.graph_overrides.iter()),
        );
    for graph in graphs {
        for node in &graph.node_intrinsics {
            for slot in node.process_chain.iter().flat_map(|chain| chain.slots.iter()) {
                if let Some(source) = &slot.expr_source {
                    sources.insert(source.clone());
                }
            }
        }
    }
    sources
}

/// Recompile every expr body the project holds and register the classes
/// (spec §2.1 "on load the host recompiles every expr source it finds").
/// Identical bodies share one class. Returns the bodies that failed, with
/// their errors; their slots keep their stored class name and do nothing
/// until the body is fixed.
pub fn sync_expr_process_classes(
    state: &crate::sequencer::SequencerState,
) -> Vec<(String, ExprCompileError)> {
    let sources = state.with_scenes(expr_sources_in_scenes);
    let mut failures = Vec::new();
    for source in sources {
        if let Some(class) = expr_class_name_for_source(&source) {
            if class == crate::process::EXPR_PROCESS_CLASS || state.has_expr_process_def(&class) {
                continue;
            }
        }
        if let Err(error) = compile_and_register_expr_source(state, &source) {
            failures.push((source, error));
        }
    }
    failures
}

/// Re-derive the class of every expr slot in `chain` from its stored body
/// and retarget wires into renamed slots. The class name is derived data;
/// this keeps a project saved under an older hash scheme consistent.
/// Returns true when anything changed.
pub fn rederive_expr_class_names_in_chain(chain: &mut crate::process::TrackProcessChain) -> bool {
    let mut renames: BTreeMap<u64, String> = BTreeMap::new();
    for slot in &mut chain.slots {
        let Some(source) = slot.expr_source.as_deref() else {
            continue;
        };
        let Some(class) = expr_class_name_for_source(source) else {
            continue;
        };
        if slot.class_name != class {
            slot.class_name = class.clone();
            renames.insert(slot.instance_id.0, class);
        }
    }
    if renames.is_empty() {
        return false;
    }
    let rename = |target: &mut crate::process::ParamTarget| {
        if let crate::process::ParamTarget::ProcessInlet { process, instance_id: Some(id), .. } = target {
            if let Some(class) = renames.get(&id.0) {
                *process = class.clone();
            }
        }
    };
    for slot in &mut chain.slots {
        for target in slot.bindings.values_mut().flatten() {
            rename(target);
        }
        for entries in slot.fanout.values_mut() {
            for entry in entries {
                rename(&mut entry.target);
            }
        }
    }
    true
}

/// The `{:ok :class :inlets :removed :error :span}` map the set-source
/// natives return.
/// Where a byte offset of `source` sits as a 0-based `(line, column)`,
/// the column counted in chars: what an editor cursor or a text style wants.
fn expr_source_position(source: &str, byte: usize) -> (usize, usize) {
    let byte = byte.min(source.len());
    let byte = (0..=byte).rev().find(|&b| source.is_char_boundary(b)).unwrap_or(0);
    let before = &source[..byte];
    let line = before.matches('\n').count();
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    (line, before[line_start..].chars().count())
}

/// The commit result map. `source` is the body that was committed, so a
/// span can also be reported as `:where (line column end-line end-column)`
/// (0-based, columns in chars) for the edit buffer's error highlight.
pub(in crate::lisp_host) fn expr_set_result_value(
    ok: bool,
    class: &str,
    inlets: &[String],
    removed: &[String],
    error: Option<&ExprCompileError>,
    source: &str,
) -> EValue {
    let names = |list: &[String]| {
        process_list(list.iter().map(|name| EValue::String(name.clone())))
    };
    let mut map = HashMap::new();
    let mut put = |key: &str, value: EValue| {
        map.insert(key.to_string(), Rc::new(RefCell::new(value)));
    };
    put("ok", EValue::Bool(ok));
    put("class", EValue::String(class.to_string()));
    put("inlets", names(inlets));
    put("removed", names(removed));
    put(
        "error",
        error.map(|error| EValue::String(error.message.clone())).unwrap_or(EValue::Nil),
    );
    put(
        "span",
        error
            .and_then(|error| error.span)
            .map(|(start, end)| {
                process_list([EValue::Number(start as f64), EValue::Number(end as f64)])
            })
            .unwrap_or(EValue::Nil),
    );
    put(
        "where",
        error
            .and_then(|error| error.span)
            .map(|(start, end)| {
                let (line, column) = expr_source_position(source, start);
                let (end_line, end_column) = expr_source_position(source, end.max(start));
                process_list([
                    EValue::Number(line as f64),
                    EValue::Number(column as f64),
                    EValue::Number(end_line as f64),
                    EValue::Number(end_column as f64),
                ])
            })
            .unwrap_or(EValue::Nil),
    );
    EValue::Map(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inlets(source: &str) -> Vec<String> {
        analyze_expr_source(source).expect("analyze").inlets
    }

    /// `:where` for the edit buffer: 0-based line and char column of a byte
    /// offset, counting chars (not bytes) before it on its line.
    #[test]
    fn source_positions_are_lines_and_char_columns() {
        let source = "(+ x\n   (sinn \u{b7}y))";
        assert_eq!(expr_source_position(source, 0), (0, 0));
        assert_eq!(expr_source_position(source, 4), (0, 4));
        assert_eq!(expr_source_position(source, 9), (1, 4));
        let dot = source.find('\u{b7}').unwrap();
        assert_eq!(expr_source_position(source, dot + 2), (1, 10), "one char for a two-byte dot");
        assert_eq!(expr_source_position(source, dot + 1), (1, 9), "mid-char offsets round down");
        assert_eq!(expr_source_position(source, 999), (1, 13));
    }

    #[test]
    fn free_argument_symbols_become_inlets_in_order() {
        assert_eq!(inlets("(sin (* x rate))"), vec!["x", "rate"]);
        assert_eq!(inlets("(+ a (* b a) c)"), vec!["a", "b", "c"]);
    }

    #[test]
    fn let_and_lambda_bindings_are_not_inlets() {
        assert_eq!(inlets("(let ((k 2) (m (* k gain))) (* m k))"), vec!["gain"]);
        assert_eq!(inlets("(let ((f (lambda (v) (* v depth)))) (f x))"), vec!["depth", "x"]);
        // A name free before its binding is still an inlet; the bound one is not.
        assert_eq!(inlets("(+ x (let ((x 1)) x))"), vec!["x"]);
    }

    #[test]
    fn context_variables_and_literals_are_not_inlets() {
        let analysis = analyze_expr_source("(+ x $foo true)").expect("analyze");
        assert_eq!(analysis.inlets, vec!["x"]);
        assert_eq!(analysis.context_vars, vec!["$foo"]);
        let error = compile_expr_source("(+ x $foo)").expect_err("unknown context var");
        assert!(error.message.contains("$foo"), "{error:?}");
        // Hex literals are numbers (eseq-waa9.14), never an `xACE1` inlet.
        assert_eq!(inlets("(bit-and 0xACE1 mask)"), vec!["mask"]);
    }

    #[test]
    fn unknown_function_position_symbol_is_an_error_with_span() {
        let error = compile_expr_source("(sinn x)").expect_err("typo in function position");
        assert!(error.message.contains("sinn"), "{error:?}");
        assert_eq!(error.span, Some((1, 5)));
        // Argument-position typos are just inlets.
        assert_eq!(inlets("(* x rtae)"), vec!["x", "rtae"]);
    }

    #[test]
    fn locals_named_like_natives_and_definitions_are_rejected() {
        // A local may shadow a global function it does not call (eseq-waa9.13);
        // binding and calling the same name is refused.
        assert!(compile_expr_source("(let ((neuron 3)) (* neuron x))").is_ok());
        let error = compile_expr_source("(let ((neuron 3)) (neuron x))").expect_err("shadow + call");
        assert!(error.message.contains("neuron"), "{error:?}");
        let error = compile_expr_source("(def y 3)").expect_err("def");
        assert!(error.message.contains("def"), "{error:?}");
        let error = compile_expr_source("(set! x 3)").expect_err("set! on an inlet");
        assert!(error.message.contains("set!"), "{error:?}");
    }

    #[test]
    fn deep_nesting_is_rejected_before_parsing() {
        let deep = format!("{}x{}", "(+ 1 ".repeat(EXPR_MAX_NESTING + 1), ")".repeat(EXPR_MAX_NESTING + 1));
        let error = compile_expr_source(&deep).expect_err("too deep");
        assert!(error.message.contains("nests"), "{error:?}");
        let ok = format!("{}x{}", "(+ 1 ".repeat(EXPR_MAX_NESTING), ")".repeat(EXPR_MAX_NESTING));
        assert!(compile_expr_source(&ok).is_ok());
    }

    #[test]
    fn identical_bodies_share_one_class_and_empty_bodies_are_plain_expr() {
        let a = compile_expr_source("(sin (* x rate))").expect("compile");
        let b = compile_expr_source("  ; comment\n(sin   (* x\n rate))").expect("compile");
        assert_eq!(a.class_name, b.class_name);
        assert!(crate::process::is_expr_process_class(&a.class_name));
        let c = compile_expr_source("(sin (* x rate 2))").expect("compile");
        assert_ne!(a.class_name, c.class_name);
        let def = a.def.expect("def");
        assert_eq!(def.inlets.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), vec!["x", "rate"]);
        assert!(def.inlets.iter().all(|i| i.lane && matches!(i.kind, crate::process::ProcessInletKind::Float)));
        assert!(def.ports.iter().any(|p| p.name == "wire" && p.is_connectable()));
        assert!(def.ports.iter().any(|p| p.name == "out" && p.is_mappable()));
        let empty = compile_expr_source(" ; nothing yet\n").expect("empty");
        assert_eq!(empty.class_name, crate::process::EXPR_PROCESS_CLASS);
        assert!(empty.def.is_none());
    }

    #[test]
    fn scoping_edge_cases_derive_the_right_inlets() {
        assert_eq!(inlets("(let ((x 1)) (+ x y))"), vec!["y"]);
        // Nested shadowing: the inner let's value sees the outer binding,
        // and a name bound only inside one branch is free in the other.
        assert_eq!(inlets("(let ((a 1)) (+ (let ((a (* a 2)) (b a)) b) a c))"), vec!["c"]);
        assert_eq!(inlets("(+ (let ((q 1)) q) q)"), vec!["q"]);
        assert_eq!(inlets("(lambda (a) (* a k))"), vec!["k"]);
        assert_eq!(inlets("(if c a b)"), vec!["c", "a", "b"]);
        // A let-bound lambda used as a head is local, its body's free names are inlets.
        assert_eq!(inlets("(let ((f (lambda (v) (+ v off)))) (f (f x)))"), vec!["off", "x"]);
        // Keywords, strings, numbers, literals and quoted data are never inlets.
        assert_eq!(inlets("(list :k \"s\" 1 nil true false 'sym '(p q))"), Vec::<String>::new());
        assert_eq!(inlets("(and (> x 0) (or y z))"), vec!["x", "y", "z"]);
        // Rest parameters bind the name after the marker.
        assert_eq!(inlets("((lambda (a & r) (+ a (len r) w)) 1 2)"), vec!["w"]);
    }

    #[test]
    fn forms_the_vm_cannot_run_are_rejected_at_commit() {
        // eseqlisp has no `fn` lambda: `((fn (a) a) 1)` is UnknownVariable a.
        let error = compile_expr_source("((fn (a) (* a k)) 2)").expect_err("fn");
        assert!(error.message.contains("lambda"), "{error:?}");
        // The VM binds plain symbols only.
        assert!(compile_expr_source("(let (((a b) (list 1 2))) (+ a b))").is_err());
        assert!(compile_expr_source("((lambda a a) 1)").is_err());
        // An inlet named like a rest marker would become one in the run lambda.
        assert!(compile_expr_source("(+ x &)").is_err());
        // Quoted data survives the round trip through the printed run body.
        let compiled = compile_expr_source("(nth '(1 5 9) i)").expect("quoted list");
        assert_eq!(compiled.inlets, vec!["i"]);
    }

    fn port_names(source: &str) -> Vec<String> {
        compile_expr_source(source)
            .expect("compile")
            .def
            .expect("def")
            .ports
            .iter()
            .map(|port| port.name.clone())
            .collect()
    }

    fn run_source(source: &str) -> String {
        compile_expr_source(source)
            .expect("compile")
            .def
            .expect("def")
            .run_source
            .expect("run source")
    }

    #[test]
    fn context_variables_compile_to_host_reads_and_are_read_only() {
        let analysis = analyze_expr_source("(+ $note $n x (* $vel $prev))").expect("analyze");
        assert_eq!(analysis.inlets, vec!["x"]);
        assert_eq!(analysis.context_vars, vec!["$note", "$n", "$vel", "$prev"]);
        for var in EXPR_CONTEXT_VARS {
            assert!(compile_expr_source(&format!("(+ {var} 1)")).is_ok(), "{var}");
        }
        assert_eq!(EXPR_CONTEXT_VARS.len(), EXPR_CONTEXT_VAR_DOCS.len());
        // Payload / transport reads lower to one native call each.
        let run = run_source("(+ $note $vel $dur $delay $beat $phase $reset)");
        for index in 0..EXPR_NATIVE_CONTEXT_VARS.len() {
            assert!(run.contains(&format!("({EXPR_CONTEXT_NATIVE} {index})")), "{index}: {run}");
        }
        assert!(!run.contains('$'), "no $ symbol survives: {run}");
        // $n / $prev are hidden state cells, declared only when read.
        let state = |source: &str| -> Vec<String> {
            compile_expr_source(source).expect("compile").def.expect("def").state
                .iter().map(|cell| cell.name.clone()).collect()
        };
        assert_eq!(state("(+ $prev $n)"), vec!["$n", "$prev"]);
        assert_eq!(state("(+ $prev 1)"), vec!["$prev"]);
        assert!(state("(+ $note 1)").is_empty());
        // Read-only: no set!, no binding.
        let error = compile_expr_source("(set! $prev 3)").expect_err("set! on $");
        assert!(error.message.contains("read-only"), "{error:?}");
        assert!(compile_expr_source("(let (($n 1)) $n)").is_err());
        assert!(compile_expr_source("(+ $nope 1)").is_err());
        // Quoted `$` names are data.
        assert!(run_source("(nth '($note) 0)").contains("$note"));
    }

    #[test]
    fn direct_writes_declare_the_ports_they_use_and_evaluate_to_nil() {
        assert_eq!(port_names("(+ x 1)"), vec!["out", "wire"]);
        assert_eq!(port_names("(if (> $n 3) (veto!))"), vec!["out", "wire"]);
        assert_eq!(port_names("(reset! 1)"), vec!["out", "wire"]);
        // Canonical order, whatever order the body uses them in.
        assert_eq!(
            port_names("(let ((a 1)) (if a (do (dur! 1) (xpose! 2)) (delay! 1)) (vel! 0.5))"),
            vec!["out", "wire", "delay!", "xpose!", "vel!", "dur!"]
        );
        let analysis = analyze_expr_source("(do (xpose! 2) (veto!) (xpose! 1) (reset!))").expect("analyze");
        assert_eq!(analysis.writes, vec!["xpose!", "veto!", "reset!"]);
        // The set of writes is part of the class.
        let class = |source: &str| compile_expr_source(source).expect("compile").class_name;
        assert_ne!(class("(do (xpose! x) nil)"), class("(do (delay! x) nil)"));
        // Each write lowers to its verb and nil, so a write-only body sends nothing.
        let run = run_source("(do (xpose! 2) (vel! v) (delay! d) (dur! 1) (veto!) (reset! :a))");
        for lowered in [
            "(do (target-add! :xpose! 2) nil)",
            "(do (target-set! :vel! v) nil)",
            "(do (target-add! :delay! d) nil)",
            "(do (target-set! :dur! 1) nil)",
            "(do (veto!) nil)",
            "(do (graph-reset! :a) nil)",
        ] {
            assert!(run.contains(lowered), "{lowered} in {run}");
        }
        // Arity is checked at commit; `(vel! event v)` is the ratchet mutator.
        for bad in ["(veto! 1)", "(delay!)", "(xpose! 1 2)", "(reset! 1 2)", "(dur! 1 2 3)"] {
            let error = compile_expr_source(bad).expect_err(bad);
            assert!(error.span.is_some(), "{bad}: {error:?}");
        }
        assert_eq!(port_names("(ratchet! 2 (lambda (i e) (vel! e 0.5)))"), vec!["out", "wire"]);
        // Write verbs cannot be rebound.
        assert!(compile_expr_source("(let ((xpose! 1)) xpose!)").is_err());
    }

    #[test]
    fn argument_names_that_are_scheduler_functions_become_shadowing_inlets() {
        let analysis = analyze_expr_source("(* vel 2)").expect("analyze");
        assert_eq!(analysis.inlets, vec!["vel"]);
        assert_eq!(analysis.shadowing, vec!["vel"]);
        let analysis = analyze_expr_source("(+ (* in k) mod)").expect("analyze");
        assert_eq!(analysis.inlets, vec!["in", "k", "mod"]);
        assert_eq!(analysis.shadowing, vec!["in", "mod"]);
        // Used both as an inlet and as a function: an error on the inlet.
        let error = compile_expr_source("(+ (vel x) vel)").expect_err("clash");
        assert!(error.message.contains("`vel` is used both as an inlet and as a function"), "{error:?}");
        assert_eq!(error.span, Some((11, 14)), "the argument occurrence");
        assert!(compile_expr_source("(+ mod (mod 5 2))").is_err());
        // Special forms and opcode heads never become inlets.
        assert_eq!(analyze_expr_source("(+ x max)").expect("analyze").inlets, vec!["x"]);
        // Names the run body calls itself cannot be inlets.
        assert!(compile_expr_source("(+ target-add! 1)").is_err());
        assert!(compile_expr_source("(+ __expr-send! 1)").is_err());
        assert!(compile_expr_source("(+ xpose! 1)").is_err());
        // The inlet binding reads `(in :name)` outside the body's scope, so
        // an inlet named `in` is bound through the global `in`.
        let run = run_source("(* in k)");
        assert!(run.starts_with("((lambda (in k) "), "{run}");
        assert!(run.ends_with("(in :in) (in :k))"), "{run}");
    }

    #[test]
    fn shadowing_and_write_dispatch_edge_cases() {
        // A local is never an inlet; one named like a global is refused.
        assert_eq!(analyze_expr_source("(let ((x 2)) (* x y))").expect("analyze").inlets, vec!["y"]);
        assert!(compile_expr_source("(let ((vel 2)) vel)").is_ok());
        // Inlet in one branch, function in the other: a clash either way round.
        assert!(compile_expr_source("(if c vel (vel 1))").is_err());
        assert!(compile_expr_source("(if c (vel 1) vel)").is_err());
        // A lambda parameter named like the ratchet event keeps the
        // two-argument mutator form and declares no write port.
        assert_eq!(port_names("(ratchet! 2 (lambda (i ev) (dur! ev 0.5)))"), vec!["out", "wire"]);
        // The one-argument write inside a lambda is still a direct write.
        assert_eq!(port_names("((lambda (a) (vel! a)) 0.5)"), vec!["out", "wire", "vel!"]);
        // Snapshot semantics are the lowering's: the write is a separate
        // statement and the `$note` read still goes through the host.
        let run = run_source("(do (xpose! 3) $note)");
        assert!(run.contains("(do (target-add! :xpose! 3) nil) (__expr-ctx 0)"), "{run}");
    }

    fn state_cells(source: &str) -> Vec<(String, crate::process::ProcessLiteral)> {
        compile_expr_source(source)
            .expect("compile")
            .def
            .expect("def")
            .state
            .iter()
            .map(|cell| (cell.name.clone(), cell.initial.clone()))
            .collect()
    }

    const LFSR: &str = "(state s 0xACE1)\n(set! s (bit-xor (shr s 1)\n                 (if (= (bit-and s 1) 1) taps 0)))\n(delay! (* grain (bit-and s 7)))";

    #[test]
    fn state_forms_declare_in_place_cells_that_are_not_inlets() {
        let analysis = analyze_expr_source(LFSR).expect("analyze");
        assert_eq!(analysis.inlets, vec!["taps", "grain"]);
        assert_eq!(analysis.state, vec![("s".to_string(), 44257.0)]);
        assert_eq!(state_cells(LFSR), vec![("s".to_string(), crate::process::ProcessLiteral::Number(44257.0))]);
        let run = run_source(LFSR);
        // Not a run-lambda parameter (whose end-of-run store would clobber
        // the in-place writes); reads and writes go through the cell native.
        assert!(run.starts_with("((lambda (taps grain) "), "{run}");
        assert!(run.contains("(__expr-cell 1 :s (bit-xor (shr (__expr-cell 0 :s) 1)"), "{run}");
        assert!(!run.contains("__process-state"), "{run}");
        // Reset restores the initial value before the body.
        assert!(run.contains("(if (reset-fired?) (do (__expr-cell 1 :s 44257)) nil)"), "{run}");
        // Declaration order is free at the top level; init defaults to 0.
        assert_eq!(analyze_expr_source("(+ a b) (state a)").expect("analyze").inlets, vec!["b"]);
        assert_eq!(state_cells("(state a) a"), vec![("a".to_string(), crate::process::ProcessLiteral::Number(0.0))]);
        assert_eq!(state_cells("(state a -0x10) a"), vec![("a".to_string(), crate::process::ProcessLiteral::Number(-16.0))]);
        // A body of state forms only sends nothing.
        assert!(run_source("(state a 1)").contains("(__expr-send! nil)"));
    }

    #[test]
    fn set_on_a_state_name_is_rewritten_at_any_depth() {
        let run = run_source("(state s 0) (let ((k 2)) (if (> k 1) (do (set! s (+ s k)) nil))) ((lambda (v) (set! s v)) 3) s");
        assert_eq!(run.matches("(__expr-cell 1 :s").count(), 3, "two body writes + the reset: {run}");
        assert!(!run.contains("set!"), "every set! targets the cell: {run}");
        // set! on a let local stays a plain set!.
        assert!(run_source("(state s 0) (let ((a 1)) (set! a 2) (+ a s))").contains("(set! a 2)"));
    }

    #[test]
    fn state_declaration_errors_are_commit_errors_with_spans() {
        for (source, needle) in [
            ("(let ((a 1)) (state s 0))", "top level"),
            ("(state s x)", "number literal"),
            ("(state s (+ 1 2))", "number literal"),
            ("(state s 1) (state s 2)", "twice"),
            ("(state $n 0)", "context variable"),
            ("(state if 0)", "built-in"),
            ("(state veto! 0)", "built-in"),
            ("(state __h0-prev 0)", "built-in"),
            ("(state (a) 0)", "state expects"),
            ("(state)", "state expects"),
            ("(state s 0) (let ((s 1)) s)", "cannot rebind"),
            ("(state s 0) ((lambda (s) s) 1)", "cannot rebind"),
            ("(state vel 0) (+ vel (vel 1))", "also called"),
        ] {
            let error = compile_expr_source(source).expect_err(source);
            assert!(error.message.contains(needle), "{source}: {error:?}");
            assert!(error.span.is_some(), "{source}: {error:?}");
        }
        // A state name shadowing a global it never calls is fine.
        assert!(compile_expr_source("(state vel 0) (set! vel (+ vel 1)) vel").is_ok());
    }

    #[test]
    fn locals_may_shadow_global_functions_they_do_not_call() {
        assert!(compile_expr_source("(let ((vel 2)) (* vel x))").is_ok());
        assert!(compile_expr_source("((lambda (count) (+ count 1)) 2)").is_ok());
        let error = compile_expr_source("(let ((vel 2)) (+ vel (vel 1)))").expect_err("bound + called");
        assert!(
            error.message.contains("`vel` is bound in the body and also called as a function"),
            "{error:?}"
        );
        assert_eq!(error.span, Some((7, 10)), "the binding");
        // Also when the call is outside the local's scope.
        assert!(compile_expr_source("(+ (vel 1) (let ((vel 2)) vel))").is_err());
        // A local named like a helper, with the helper also called.
        assert!(compile_expr_source("(+ (prev x) (let ((prev 1)) prev))").is_err());
        // Special forms, opcode heads, literals and reserved names stay unbindable.
        for source in ["(let ((if 1)) 1)", "(let ((max 1)) 1)", "(let ((nil 1)) 1)", "(let ((veto! 1)) 1)"] {
            assert!(compile_expr_source(source).is_err(), "{source}");
        }
    }

    #[test]
    fn stateful_helpers_own_one_hidden_cell_per_call_site() {
        let source = "(+ (prev x) (prev x) (delta x) (integ x) (sh g x) (slew x 0.5) (count 4) (or (every 3 x) 0))";
        let analysis = analyze_expr_source(source).expect("analyze");
        assert_eq!(analysis.inlets, vec!["x", "g"]);
        assert_eq!(
            analysis.helpers,
            vec!["prev", "prev", "delta", "integ", "sh", "slew", "count", "every"]
        );
        let cells: Vec<String> = state_cells(source).into_iter().map(|(name, _)| name).collect();
        assert_eq!(
            cells,
            vec![
                "__h0-prev", "__h1-prev", "__h2-delta", "__h3-integ", "__h4-sh", "__h5-slew",
                "__h6-count", "__h7-every"
            ]
        );
        let run = run_source(source);
        for (op, cell) in [(2, "__h0-prev"), (2, "__h1-prev"), (3, "__h2-delta"), (8, "__h6-count"), (7, "__h7-every")] {
            assert!(run.contains(&format!("(__expr-cell {op} :{cell} ")), "{op} {cell}: {run}");
        }
        // Nested sites are numbered in pre-order; user state comes first.
        let cells: Vec<String> =
            state_cells("(state s 1) (prev (delta s))").into_iter().map(|(name, _)| name).collect();
        assert_eq!(cells, vec!["s", "__h0-prev", "__h1-delta"]);
        // Arity, lambdas, and a helper name used as an inlet.
        for (source, needle) in [
            ("(prev)", "takes 1 argument"),
            ("(slew x)", "takes 2 arguments"),
            ("(count 1 2)", "takes 1 argument"),
            ("((lambda (v) (prev v)) x)", "inside a lambda"),
            ("(let ((f (lambda () (count 4)))) (f))", "inside a lambda"),
            ("(+ prev (prev x))", "used both as an inlet and as a function"),
        ] {
            let error = compile_expr_source(source).expect_err(source);
            assert!(error.message.contains(needle), "{source}: {error:?}");
        }
        // Quoted helper calls are data.
        assert!(analyze_expr_source("(nth '((prev x)) 0)").expect("analyze").helpers.is_empty());
    }

    #[test]
    fn state_and_helper_rewrites_are_deterministic_per_body() {
        let a = compile_expr_source(LFSR).expect("compile").def.expect("def");
        let b = compile_expr_source(&format!("  ; lfsr\n{}", LFSR.replace('\n', "   "))).expect("compile").def.expect("def");
        assert_eq!(a.name, b.name, "whitespace/comments do not change the class");
        assert_eq!(a.run_source, b.run_source);
        assert_eq!(a.state, b.state);
        // Cell names and init values are part of the body, hence the hash.
        let class = |source: &str| expr_class_name_for_source(source).expect("parses");
        assert_ne!(class("(state s 1) s"), class("(state s 2) s"));
        assert_ne!(class("(state s 1) s"), class("(state t 1) t"));
    }

    // ---- eseq-waa9.15: threading, constants, shaping helpers (spec §6.1) ----

    #[test]
    fn threading_expands_before_analysis_so_helpers_get_cells_and_arity() {
        // The spec example reads in chain order and derives the same inlets
        // as the nested form.
        let source = "(-> x (* rate) sin (scale -1 1 0 4))";
        assert_eq!(inlets(source), vec!["x", "rate"]);
        let run = run_source(source);
        assert!(
            run.contains("(__expr-fn 3 (sin (* x rate)) -1 1 0 4)"),
            "scale lowered around the threaded value: {run}"
        );
        // ->> threads last.
        assert!(run_source("(->> x (- 1) (* 2))").contains("(* 2 (- 1 x))"));
        // A stateful helper through -> gets its arity check and its cell.
        let analysis = analyze_expr_source("(-> x prev)").expect("(-> x prev)");
        assert_eq!(analysis.helpers, vec!["prev"]);
        assert_eq!(analysis.inlets, vec!["x"]);
        let cells: Vec<String> =
            state_cells("(-> x (slew 0.5) prev)").into_iter().map(|(name, _)| name).collect();
        assert_eq!(cells, vec!["__h0-prev", "__h1-slew"], "pre-order on the expanded form");
        assert!(run_source("(-> x prev)").contains("(__expr-cell 2 :__h0-prev x)"));
        // ... and the arity error still fires on the threaded result.
        let error = compile_expr_source("(-> x (slew 0.5 0.1))").expect_err("slew with 3 args");
        assert!(error.message.contains("takes 2 arguments"), "{error:?}");
        assert!(error.span.is_some());
        // A threaded stage that is itself a -> form nests like the VM's desugar.
        // (-> x (-> (+ 1))) = (-> x (+ 1)) = (+ x 1), as the compiler's desugar.
        assert!(run_source("(-> x (-> (+ 1)))").contains("(__expr-send! (+ x 1))"));
        assert_eq!(inlets("(-> x (-> (+ 1)) (* k))"), vec!["x", "k"]);
        // Quoted threading is data.
        assert!(run_source("(nth '((-> a b)) 0)").contains("(-> a b)"));
        // Bad shapes are commit errors.
        assert!(compile_expr_source("(->)").is_err());
        assert!(compile_expr_source("(-> x 3)").is_err());
        // The class hash stays on the authored body.
        let class = |source: &str| expr_class_name_for_source(source).expect("parses");
        assert_eq!(class("(-> x sin)"), compile_expr_source("(-> x sin)").unwrap().class_name);
        // Expansion deepens nesting: the expanded depth is held to the limit.
        let long = format!("(-> x {})", "(+ 1) ".repeat(EXPR_MAX_NESTING + 2));
        let error = compile_expr_source(&long).expect_err("too deep once expanded");
        assert!(error.message.contains("nests"), "{error:?}");
        assert!(compile_expr_source(&format!("(-> x {})", "(+ 1) ".repeat(8))).is_ok());
    }

    #[test]
    fn pi_and_tau_are_constants_not_inlets() {
        assert_eq!(inlets("(* tau (+ x pi))"), vec!["x"]);
        let run = run_source("(* tau x pi)");
        assert!(run.contains(&format!("{}", std::f64::consts::TAU)), "{run}");
        assert!(run.contains(&format!("{}", std::f64::consts::PI)), "{run}");
        assert!(!run.contains(" tau") && !run.contains(" pi"), "{run}");
        assert!(compile_expr_source("(let ((pi 3)) pi)").is_err(), "constants are not bindable");
        assert!(compile_expr_source("(state tau 1)").is_err());
        assert!(compile_expr_source("(pi)").is_err(), "not a function");
    }

    #[test]
    fn shaping_helpers_lower_to_one_native_and_check_arity() {
        let analysis = analyze_expr_source(
            "(+ (quant x 1) (fold x 0 1) (wrap x 0 1) (scale x 0 1 2 3) (clip x 0 1) (euclid 3 8 $n) (choose a b) (sine p) (tri p) (saw p) (sqr p) (sqr p 0.25) (unipolar x) (bipolar x))",
        )
        .expect("analyze");
        assert_eq!(analysis.inlets, vec!["x", "a", "b", "p"]);
        assert!(analysis.helpers.is_empty(), "pure helpers keep no cells");
        let run = run_source("(+ (quant x 1) (choose a 2) (wrap x 0 1))");
        assert!(run.contains("(__expr-fn 0 x 1)"), "{run}");
        assert!(run.contains("(__expr-choose a 2)"), "{run}");
        assert!(run.contains("(__expr-fn 2 x 0 1)"), "wrap goes through the expr helper too: {run}");
        for (source, needle) in [
            ("(quant x)", "takes 2 arguments"),
            ("(scale x 0 1 2)", "takes 5 arguments"),
            ("(sqr p 0.5 1)", "takes 1 to 2 arguments"),
            ("(choose)", "at least 1 argument"),
            ("(euclid 3 8)", "takes 3 arguments"),
            ("(+ fold (fold x 0 1))", "used both as an inlet and as a function"),
            ("(let ((quant 1)) (quant x 1))", "also called as a function"),
        ] {
            let error = compile_expr_source(source).expect_err(source);
            assert!(error.message.contains(needle), "{source}: {error:?}");
        }
        // Not called, a helper name is an ordinary inlet or local.
        assert_eq!(inlets("(+ scale 1)"), vec!["scale"]);
        assert!(compile_expr_source("(let ((saw 2)) (* saw x))").is_ok());
    }

    fn pure(op: u8, args: &[f64]) -> f64 {
        expr_pure_fn(op, args).expect("pure fn")
    }

    #[test]
    fn shaping_helpers_compute_their_documented_values() {
        use expr_fn_op as f;
        // euclid 3 8 is x..x..x. and i wraps mod n (negatives too).
        let pattern: String = (0..8).map(|i| if pure(f::EUCLID, &[3.0, 8.0, i as f64]) == 1.0 { 'x' } else { '.' }).collect();
        assert_eq!(pattern, "x..x..x.");
        assert_eq!(pure(f::EUCLID, &[3.0, 8.0, 11.0]), 1.0, "11 mod 8 = 3");
        assert_eq!(pure(f::EUCLID, &[3.0, 8.0, -5.0]), 1.0, "-5 mod 8 = 3");
        let count = |k: f64, n: usize| (0..n).filter(|i| pure(f::EUCLID, &[k, n as f64, *i as f64]) == 1.0).count();
        assert_eq!(count(5.0, 16), 5);
        assert_eq!(count(0.0, 8), 0);
        assert_eq!(count(9.0, 8), 8, "k clamps to n");
        assert_eq!(pure(f::EUCLID, &[3.0, 0.0, 1.0]), 0.0, "n < 1 never hits");
        // quant: nearest multiple, negatives symmetric, halves away from 0.
        assert_eq!(pure(f::QUANT, &[2.6, 1.0]), 3.0);
        assert_eq!(pure(f::QUANT, &[-2.6, 1.0]), -3.0);
        assert_eq!(pure(f::QUANT, &[-7.0, 4.0]), -8.0);
        assert_eq!(pure(f::QUANT, &[-1.5, 1.0]), -2.0);
        assert_eq!(pure(f::QUANT, &[-0.4, 1.0]).to_bits(), 0.0f64.to_bits(), "no -0");
        assert_eq!(pure(f::QUANT, &[0.37, 0.25]), 0.25);
        assert_eq!(pure(f::QUANT, &[0.37, -0.25]), 0.25, "step sign ignored");
        assert_eq!(pure(f::QUANT, &[0.37, 0.0]), 0.37, "step 0 passes x");
        // fold: ping-pong, both edges inclusive.
        for (x, expected) in [(0.0, 0.0), (1.0, 1.0), (1.5, 0.5), (2.0, 0.0), (2.25, 0.25), (-0.5, 0.5), (-1.0, 1.0), (0.3, 0.3)] {
            assert!((pure(f::FOLD, &[x, 0.0, 1.0]) - expected).abs() < 1e-12, "fold {x}");
        }
        assert_eq!(pure(f::FOLD, &[7.0, 2.0, -2.0]), -1.0, "swapped bounds");
        assert_eq!(pure(f::FOLD, &[7.0, 3.0, 3.0]), 3.0, "empty range gives lo");
        // wrap: half-open, hi wraps to lo, negatives wrap up.
        for (x, expected) in [(0.0, 0.0), (1.0, 0.0), (1.25, 0.25), (-0.25, 0.75), (-1.0, 0.0), (0.5, 0.5)] {
            assert!((pure(f::WRAP, &[x, 0.0, 1.0]) - expected).abs() < 1e-12, "wrap {x}");
        }
        assert_eq!(pure(f::WRAP, &[13.0, 10.0, 12.0]), 11.0);
        assert_eq!(pure(f::WRAP, &[5.0, 2.0, 2.0]), 2.0);
        // scale / clip.
        assert_eq!(pure(f::SCALE, &[0.0, -1.0, 1.0, 0.0, 4.0]), 2.0);
        assert_eq!(pure(f::SCALE, &[2.0, 0.0, 1.0, 10.0, 20.0]), 30.0, "not clamped");
        assert_eq!(pure(f::SCALE, &[0.5, 1.0, 1.0, 3.0, 4.0]), 3.0, "empty input range gives out-lo");
        assert_eq!(pure(f::CLIP, &[5.0, 0.0, 1.0]), 1.0);
        assert_eq!(pure(f::CLIP, &[-5.0, 1.0, 0.0]), 0.0, "swapped bounds");
        // Shapers of a 0..1 phase, as alez.sig defines them.
        assert!((pure(f::SINE, &[0.0]) - 0.5).abs() < 1e-12);
        assert!((pure(f::SINE, &[0.25]) - 1.0).abs() < 1e-12);
        assert!(pure(f::SINE, &[0.75]).abs() < 1e-12);
        assert_eq!(pure(f::TRI, &[0.0]), 0.0);
        assert_eq!(pure(f::TRI, &[0.5]), 1.0);
        assert_eq!(pure(f::TRI, &[0.25]), 0.5);
        assert_eq!(pure(f::TRI, &[1.25]), 0.5);
        assert_eq!(pure(f::SAW, &[1.25]), 0.25);
        assert_eq!(pure(f::SAW, &[-0.25]), 0.75);
        assert_eq!(pure(f::SQR, &[0.25]), 1.0);
        assert_eq!(pure(f::SQR, &[0.75]), 0.0);
        assert_eq!(pure(f::SQR, &[0.3, 0.25]), 0.0);
        assert_eq!(pure(f::SQR, &[0.2, 0.25]), 1.0);
        assert_eq!(pure(f::UNIPOLAR, &[-1.0]), 0.0);
        assert_eq!(pure(f::UNIPOLAR, &[1.0]), 1.0);
        assert_eq!(pure(f::BIPOLAR, &[0.0]), -1.0);
        assert_eq!(pure(f::BIPOLAR, &[0.75]), 0.5);
        assert!(pure(f::FOLD, &[f64::NAN, 0.0, 1.0]).is_nan(), "NaN in, NaN out");
        assert!(expr_pure_fn(99, &[1.0]).is_err());
    }

    #[test]
    fn shaping_helpers_hold_their_edges() {
        use expr_fn_op as f;
        let pattern = |k: f64, n: usize| -> String {
            (0..n).map(|i| if pure(f::EUCLID, &[k, n as f64, i as f64]) == 1.0 { 'x' } else { '.' }).collect()
        };
        // Bresenham rotation of Bjorklund's x.xx.xx. (same necklace, hit on 0).
        assert_eq!(pattern(5.0, 8), "x.x.xx.x");
        let bjorklund = "x.xx.xx.";
        assert!((0..8).any(|r| format!("{}{}", &bjorklund[r..], &bjorklund[..r]) == pattern(5.0, 8)));
        assert_eq!(pattern(0.0, 5), ".....");
        assert_eq!(pattern(5.0, 5), "xxxxx");
        for (k, n) in [(1.0, 4), (3.0, 8), (5.0, 8), (7.0, 16), (4.0, 12)] {
            assert_eq!(pure(f::EUCLID, &[k, n as f64, 0.0]), 1.0, "euclid {k} {n} hits on step 0");
        }
        assert_eq!(pattern(2.9, 4), "x.x.", "k floors");
        // fold at hi and at whole multiples of the range.
        assert_eq!(pure(f::FOLD, &[3.0, 1.0, 3.0]), 3.0);
        assert_eq!(pure(f::FOLD, &[5.0, 1.0, 3.0]), 1.0, "lo + 2·span folds back to lo");
        assert_eq!(pure(f::FOLD, &[7.0, 1.0, 3.0]), 3.0, "lo + 3·span lands on hi");
        assert_eq!(pure(f::FOLD, &[-3.0, 1.0, 3.0]), 1.0);
        // wrap negative, and a rounding-sized negative stays half-open.
        assert_eq!(pure(f::WRAP, &[-3.5, 0.0, 2.0]), 0.5);
        assert_eq!(pure(f::WRAP, &[-1e-17, 0.0, 1.0]), 0.0, "never returns hi");
        assert_eq!(pure(f::SAW, &[-1e-17]), 0.0, "never returns 1");
        // sqr duty edges.
        for p in [0.0, 0.25, 0.999, -1e-17] {
            assert_eq!(pure(f::SQR, &[p, 0.0]), 0.0, "duty 0 is always low ({p})");
            assert_eq!(pure(f::SQR, &[p, 1.0]), 1.0, "duty 1 is always high ({p})");
        }
        // scale with an empty input range.
        assert_eq!(pure(f::SCALE, &[9.0, 2.0, 2.0, -1.0, 1.0]), -1.0);
        // NaN propagates from every argument position, clip bounds included
        // (f64::clamp panics on a NaN bound).
        assert!(pure(f::CLIP, &[0.5, 0.0, f64::NAN]).is_nan());
        assert!(pure(f::CLIP, &[0.5, f64::NAN, 1.0]).is_nan());
        assert!(pure(f::SQR, &[f64::NAN]).is_nan());
        assert!(pure(f::EUCLID, &[3.0, 8.0, f64::NAN]).is_nan());
        assert!(pure(f::QUANT, &[1.0, f64::NAN]).is_nan());
        assert_eq!(pure(f::CLIP, &[5.0, f64::NEG_INFINITY, f64::INFINITY]), 5.0);
    }

    #[test]
    fn threading_errors_point_at_the_authored_source() {
        // An unknown function inside a threaded stage spans its head.
        let source = "(-> x (* 2) (sinn))";
        let error = compile_expr_source(source).expect_err("sinn is unknown");
        assert!(error.message.contains("unknown function `sinn`"), "{error:?}");
        let (start, end) = error.span.expect("span");
        assert_eq!((start, &source[start..end]), (13, "sinn"), "{error:?}");
        let source = "(-> x sinn)";
        let error = compile_expr_source(source).expect_err("bare sinn is unknown");
        let (start, end) = error.span.expect("span");
        assert_eq!((start, &source[start..end]), (6, "sinn"), "{error:?}");
        // An arity error inside a threaded stage spans the user's stage.
        let source = "(-> x (sine 1 2))";
        let error = compile_expr_source(source).expect_err("sine takes 1");
        let (start, end) = error.span.expect("span");
        assert_eq!(&source[start..end], "(sine 1 2)", "{error:?}");
        let source = "(-> x (+ 1) 3)";
        let error = compile_expr_source(source).expect_err("bad stage");
        let (start, end) = error.span.expect("span");
        assert_eq!(&source[start..end], "3", "{error:?}");
        let source = "(+ 1 (-> ))";
        let error = compile_expr_source(source).expect_err("empty ->");
        let (start, end) = error.span.expect("span");
        assert_eq!(&source[start..end], "(-> )", "{error:?}");
    }

    #[test]
    fn hash_separates_semantically_different_bodies() {
        let class = |source: &str| expr_class_name_for_source(source).expect("parses");
        assert_ne!(class("(+ x \"a\")"), class("(+ x a)"));
        assert_ne!(class("(+ x :a)"), class("(+ x a)"));
        assert_ne!(class("(+ x 0.1)"), class("(+ x 0.1000001)"));
        assert_ne!(class("(+ x \"a b\")"), class("(+ x \"a  b\")"));
        assert_ne!(class("(* x rate)"), class("(* rate x)"));
        assert_eq!(class("(+ x 1)"), class("(+ x 1.0)"));
    }
}
