//! What a DGenLisp definition comes to at given knob values, for the agent's
//! explanations: `B` at brass, 6.2 m and 0.22 m is a number, and `freqs` a
//! vector of 80. Only the part of the language that is fixed between samples
//! is evaluated here (arithmetic, comparisons, `gswitch`, `iota`, math over
//! params and constants, element-wise over vectors). Anything that reads a
//! signal (`in`, a history, an oscillator, a macro) varies per sample and is
//! reported as such, naming what it reads, rather than guessed.

use std::collections::HashMap;

use eseqlisp::parser::{ASTParser, Expression, Parser};

/// What a definition comes to.
#[derive(Debug, Clone, PartialEq)]
pub enum DgenValue {
    Scalar(f64),
    Vector(Vec<f64>),
    /// Varies per sample (or could not be evaluated): why.
    Varies(String),
}

/// A source's params with their declared defaults, in source order.
pub fn params(source: &str) -> Result<Vec<(String, f64)>, String> {
    Ok(Program::parse(source)?.params)
}

/// Evaluate the top-level `def`s named in `names` in `source`, with `knobs`
/// overriding the params' defaults (a param not in `knobs` takes its
/// `@default`) at `samplerate`.
pub fn evaluate(
    source: &str,
    knobs: &HashMap<String, f64>,
    samplerate: f64,
    names: &[String],
) -> Result<Vec<(String, DgenValue)>, String> {
    let program = Program::parse(source)?;
    let mut eval = Eval { program: &program, knobs, samplerate, memo: HashMap::new(), depth: 0 };
    Ok(names.iter().map(|name| (name.clone(), eval.name(name))).collect())
}

/// One readable line for a value: a scalar, or a vector's length, range and
/// first entries.
pub fn describe(value: &DgenValue) -> String {
    match value {
        DgenValue::Scalar(x) => number(*x),
        DgenValue::Vector(xs) => {
            let lo = xs.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let head: Vec<String> = xs.iter().take(6).map(|x| number(*x)).collect();
            let more = if xs.len() > 6 { ", …" } else { "" };
            format!("vector of {} from {} to {}: [{}{more}]", xs.len(), number(lo), number(hi), head.join(", "))
        }
        DgenValue::Varies(why) => format!("varies per sample ({why})"),
    }
}

fn number(x: f64) -> String {
    if x == x.trunc() && x.abs() < 1e7 {
        format!("{}", x as i64)
    } else if x.abs() >= 1e5 || x.abs() < 1e-3 {
        format!("{x:.4e}")
    } else {
        format!("{x:.4}").trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

struct Program {
    /// Each top-level `def`'s expression; a later definition wins.
    defs: HashMap<String, Expression>,
    params: Vec<(String, f64)>,
    macros: Vec<String>,
}

impl Program {
    fn parse(source: &str) -> Result<Self, String> {
        let tokens = Parser::new(source.to_string()).parse().map_err(|error| format!("cannot read the source: {error:?}"))?;
        let forms = ASTParser::new(tokens).parse().map_err(|error| format!("cannot parse the source: {error:?}"))?;
        let mut program = Program { defs: HashMap::new(), params: Vec::new(), macros: Vec::new() };
        for form in forms {
            let Expression::List(items) = form else { continue };
            match items.as_slice() {
                [Expression::Symbol(head), Expression::Symbol(name), value, ..] if head == "def" => {
                    program.defs.insert(name.clone(), value.clone());
                }
                [Expression::Symbol(head), Expression::Symbol(name), rest @ ..] if head == "param" => {
                    let default = rest
                        .windows(2)
                        .find_map(|pair| match pair {
                            [Expression::Symbol(attr), Expression::Number(value)] if attr == "@default" => Some(*value),
                            _ => None,
                        })
                        .unwrap_or(0.0);
                    program.params.push((name.clone(), default));
                }
                [Expression::Symbol(head), Expression::Symbol(name), ..] if head == "defmacro" => {
                    program.macros.push(name.clone());
                }
                _ => {}
            }
        }
        Ok(program)
    }
}

/// Reads that change every sample: a definition using one varies.
const SIGNAL_OPS: &[&str] = &[
    "in", "history", "make-history", "read-history", "write-history", "phasor", "noise", "latch",
    "accum", "delay", "delay1", "sah", "change", "counter", "buffer", "peek", "poke", "fft", "ifft",
];

/// More nesting than any real definition: a cycle, reported as such.
const MAX_DEPTH: usize = 200;

struct Eval<'a> {
    program: &'a Program,
    knobs: &'a HashMap<String, f64>,
    samplerate: f64,
    memo: HashMap<String, DgenValue>,
    depth: usize,
}

impl Eval<'_> {
    fn name(&mut self, name: &str) -> DgenValue {
        if let Some(value) = self.memo.get(name) {
            return value.clone();
        }
        let value = if let Some(value) = self.knobs.get(name) {
            DgenValue::Scalar(*value)
        } else if let Some((_, default)) = self.program.params.iter().find(|(param, _)| param == name) {
            DgenValue::Scalar(*default)
        } else if let Some(expr) = self.program.defs.get(name) {
            if self.depth > MAX_DEPTH {
                return DgenValue::Varies(format!("`{name}` refers to itself"));
            }
            self.depth += 1;
            let value = self.expr(expr);
            self.depth -= 1;
            value
        } else {
            match name {
                "samplerate" => DgenValue::Scalar(self.samplerate),
                "pi" => DgenValue::Scalar(std::f64::consts::PI),
                "twopi" => DgenValue::Scalar(std::f64::consts::TAU),
                _ => DgenValue::Varies(format!("`{name}` is not a top-level def or param")),
            }
        };
        self.memo.insert(name.to_string(), value.clone());
        value
    }

    fn expr(&mut self, expr: &Expression) -> DgenValue {
        match expr {
            Expression::Number(x) => DgenValue::Scalar(*x),
            Expression::Symbol(name) => self.name(name),
            Expression::List(items) => {
                let Some(Expression::Symbol(op)) = items.first() else {
                    return DgenValue::Varies("a list that is not a call".into());
                };
                // Attributes (`@name gate`) are metadata, not arguments.
                let mut args = Vec::new();
                let mut input_name = None;
                let mut rest = items[1..].iter();
                while let Some(arg) = rest.next() {
                    match arg {
                        Expression::Symbol(attr) if attr.starts_with('@') => {
                            if let (true, Some(Expression::Symbol(name))) = (attr == "@name", rest.next()) {
                                input_name = Some(name.clone());
                            }
                        }
                        _ => args.push(arg),
                    }
                }
                if op == "in" {
                    return DgenValue::Varies(match input_name {
                        Some(name) => format!("reads input `{name}`: give `{name}` a value to work it out"),
                        None => "reads an `in` input".to_string(),
                    });
                }
                self.call(op, &args)
            }
            _ => DgenValue::Varies("not a number".into()),
        }
    }

    fn call(&mut self, op: &str, args: &[&Expression]) -> DgenValue {
        if SIGNAL_OPS.contains(&op) {
            return DgenValue::Varies(format!("reads `{op}`"));
        }
        if self.program.macros.iter().any(|name| name == op) {
            return DgenValue::Varies(format!("calls macro `{op}`"));
        }
        let values: Vec<DgenValue> = args.iter().map(|arg| self.expr(arg)).collect();
        if let Some(varies) = values.iter().find(|value| matches!(value, DgenValue::Varies(_))) {
            return varies.clone();
        }
        let unary = |f: fn(f64) -> f64| match values.as_slice() {
            [value] => map1(value, f),
            _ => DgenValue::Varies(format!("`{op}` takes one argument")),
        };
        let binary = |f: fn(f64, f64) -> f64| match values.as_slice() {
            [a, b] => map2(a, b, f),
            _ => DgenValue::Varies(format!("`{op}` takes two arguments")),
        };
        let fold = |f: fn(f64, f64) -> f64, unit: f64| {
            let mut iter = values.iter();
            let first = match (iter.next(), values.len()) {
                (Some(first), 1) if op == "-" => return map1(first, |x| -x),
                (Some(first), 1) if op == "/" => return map1(first, |x| 1.0 / x),
                (Some(first), _) => first.clone(),
                (None, _) => DgenValue::Scalar(unit),
            };
            iter.fold(first, |acc, value| map2(&acc, value, f))
        };
        match op {
            "+" => fold(|a, b| a + b, 0.0),
            "*" => fold(|a, b| a * b, 1.0),
            "-" => fold(|a, b| a - b, 0.0),
            "/" => fold(|a, b| a / b, 1.0),
            "min" => fold(f64::min, f64::INFINITY),
            "max" => fold(f64::max, f64::NEG_INFINITY),
            "mod" | "%" => binary(|a, b| a.rem_euclid(b)),
            "pow" => binary(f64::powf),
            "atan2" => binary(f64::atan2),
            "lt" | "<" => binary(|a, b| truth(a < b)),
            "lte" | "<=" => binary(|a, b| truth(a <= b)),
            "gt" | ">" => binary(|a, b| truth(a > b)),
            "gte" | ">=" => binary(|a, b| truth(a >= b)),
            "eq" | "==" => binary(|a, b| truth(a == b)),
            "neq" | "!=" => binary(|a, b| truth(a != b)),
            "and" => binary(|a, b| truth(a != 0.0 && b != 0.0)),
            "or" => binary(|a, b| truth(a != 0.0 || b != 0.0)),
            "not" => unary(|a| truth(a == 0.0)),
            "floor" => unary(f64::floor),
            "ceil" => unary(f64::ceil),
            "round" => unary(f64::round),
            "abs" => unary(f64::abs),
            "sqrt" => unary(f64::sqrt),
            "exp" => unary(f64::exp),
            "log" => unary(f64::ln),
            "log10" => unary(f64::log10),
            "sin" => unary(f64::sin),
            "cos" => unary(f64::cos),
            "tan" => unary(f64::tan),
            "tanh" => unary(f64::tanh),
            "sign" => unary(f64::signum),
            "clip" | "clamp" => match values.as_slice() {
                [x, lo, hi] => map2(&map2(x, lo, f64::max), hi, f64::min),
                _ => DgenValue::Varies(format!("`{op}` takes three arguments")),
            },
            "gswitch" | "selector" if values.len() == 3 => {
                let [test, yes, no] = values.as_slice() else { unreachable!() };
                select(test, yes, no)
            }
            "iota" => match values.as_slice() {
                [DgenValue::Scalar(n)] if *n >= 0.0 && *n <= 1e6 => DgenValue::Vector((0..*n as usize).map(|i| i as f64).collect()),
                _ => DgenValue::Varies("`iota` takes a count".into()),
            },
            "sum" => match values.as_slice() {
                [DgenValue::Vector(xs)] => DgenValue::Scalar(xs.iter().sum()),
                [DgenValue::Scalar(x)] => DgenValue::Scalar(*x),
                _ => DgenValue::Varies("`sum` takes one argument".into()),
            },
            _ => DgenValue::Varies(format!("`{op}` is not evaluated here")),
        }
    }
}

/// DGenLisp's true and false.
fn truth(test: bool) -> f64 {
    if test { 1.0 } else { 0.0 }
}

fn map1(value: &DgenValue, f: fn(f64) -> f64) -> DgenValue {
    match value {
        DgenValue::Scalar(x) => DgenValue::Scalar(f(*x)),
        DgenValue::Vector(xs) => DgenValue::Vector(xs.iter().map(|x| f(*x)).collect()),
        varies => varies.clone(),
    }
}

/// Element-wise, a scalar broadcast against a vector.
fn map2(a: &DgenValue, b: &DgenValue, f: fn(f64, f64) -> f64) -> DgenValue {
    match (a, b) {
        (DgenValue::Scalar(x), DgenValue::Scalar(y)) => DgenValue::Scalar(f(*x, *y)),
        (DgenValue::Vector(xs), DgenValue::Scalar(y)) => DgenValue::Vector(xs.iter().map(|x| f(*x, *y)).collect()),
        (DgenValue::Scalar(x), DgenValue::Vector(ys)) => DgenValue::Vector(ys.iter().map(|y| f(*x, *y)).collect()),
        (DgenValue::Vector(xs), DgenValue::Vector(ys)) if xs.len() == ys.len() => {
            DgenValue::Vector(xs.iter().zip(ys).map(|(x, y)| f(*x, *y)).collect())
        }
        (DgenValue::Vector(xs), DgenValue::Vector(ys)) => {
            DgenValue::Varies(format!("vectors of {} and {} do not line up", xs.len(), ys.len()))
        }
        (DgenValue::Varies(why), _) | (_, DgenValue::Varies(why)) => DgenValue::Varies(why.clone()),
    }
}

/// `gswitch`: where the test is non-zero the first value, else the second.
fn select(test: &DgenValue, yes: &DgenValue, no: &DgenValue) -> DgenValue {
    match test {
        DgenValue::Scalar(t) => if *t != 0.0 { yes.clone() } else { no.clone() },
        DgenValue::Vector(ts) => {
            let pick = |value: &DgenValue, i: usize| match value {
                DgenValue::Scalar(x) => Some(*x),
                DgenValue::Vector(xs) => xs.get(i).copied(),
                DgenValue::Varies(_) => None,
            };
            let picked: Option<Vec<f64>> =
                ts.iter().enumerate().map(|(i, t)| if *t != 0.0 { pick(yes, i) } else { pick(no, i) }).collect();
            picked.map_or_else(|| DgenValue::Varies("vectors do not line up".into()), DgenValue::Vector)
        }
        varies => varies.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "
(def gate (in 1 @name gate))
(param material @default 0 @min 0 @max 7)
(param length @default 10 @min 0.2 @max 20 @unit m)
(param radius @default 0.1 @min 0.001 @max 0.3 @unit m)
(defmacro onset-of (trigger) (make-history previous) trigger)
(def hit (onset-of gate))
(def mat (floor (+ material 0.5)))
(def density (gswitch (lt mat 0.5) 19100 (gswitch (lt mat 1.5) 7850 8500)))
(def young (gswitch (lt mat 0.5) 2.08e11 (gswitch (lt mat 1.5) 2.0e11 1.0e11)))
(def f0 (max 220 20))
(def n (+ (iota 4) 1))
(def B (/ (* 0.61685 young radius radius) (* density length length length length f0 f0)))
(def freqs (* n f0 (/ (sqrt (+ 1 (* B n n))) (sqrt (+ 1 B)))))
(def w (min (* twopi (/ freqs samplerate)) 2.8))
(def mute (* 2 (- 1 gate)))
";

    fn at(knobs: &[(&str, f64)], names: &[&str]) -> Vec<DgenValue> {
        let knobs = knobs.iter().map(|(name, value)| (name.to_string(), *value)).collect();
        let names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
        evaluate(SOURCE, &knobs, 48000.0, &names).unwrap().into_iter().map(|(_, value)| value).collect()
    }

    #[test]
    fn knobs_pick_branches_and_defaults_fill_the_rest() {
        assert_eq!(at(&[], &["density"]), [DgenValue::Scalar(19100.0)]);
        assert_eq!(at(&[("material", 2.0)], &["density", "young"]), [DgenValue::Scalar(8500.0), DgenValue::Scalar(1.0e11)]);
        assert_eq!(params(SOURCE).unwrap()[1], ("length".to_string(), 10.0));
    }

    #[test]
    fn vectors_broadcast_and_stiffness_stretches_upper_modes_only() {
        let values = at(&[("material", 2.0), ("length", 6.2), ("radius", 0.22)], &["freqs", "B"]);
        let [DgenValue::Vector(freqs), DgenValue::Scalar(b)] = values.as_slice() else {
            panic!("freqs is a vector and B a scalar: {values:?}")
        };
        let b = *b;
        let expected_b = 0.61685 * 1.0e11 * 0.22 * 0.22 / (8500.0 * 6.2f64.powi(4) * 220.0 * 220.0);
        assert!((b - expected_b).abs() < 1e-12);
        assert_eq!(freqs.len(), 4);
        assert!((freqs[0] - 220.0).abs() < 1e-9, "mode 1 stays on the note");
        assert!(freqs[3] > 4.0 * 220.0, "upper modes stretch sharp");
    }

    #[test]
    fn signals_and_macros_vary_per_sample_and_say_why() {
        assert_eq!(at(&[], &["mute"]), [DgenValue::Varies("reads input `gate`: give `gate` a value to work it out".into())]);
        // An input given a value is a number like any knob.
        assert_eq!(at(&[("gate", 1.0)], &["mute"]), [DgenValue::Scalar(0.0)]);
        assert_eq!(at(&[], &["hit"]), [DgenValue::Varies("calls macro `onset-of`".into())]);
        assert_eq!(at(&[], &["nope"]), [DgenValue::Varies("`nope` is not a top-level def or param".into())]);
        assert_eq!(describe(&at(&[], &["n"])[0]), "vector of 4 from 1 to 4: [1, 2, 3, 4]");
    }
}
