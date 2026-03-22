//! CPU-side SDF hit testing.
//!
//! Evaluates macro-expanded SDF expressions at a point (x, y) to determine
//! which `sdf/fill` region contains the point. Returns the topmost region
//! index or -1 if no region is hit.

use crate::parser::Expression;
use std::collections::HashMap;

/// Evaluate an SDF expression tree at the given (x, y) coordinates.
/// Returns a scalar f64 (distance value) or f64::NAN on error.
fn eval_sdf_expr(expr: &Expression, vars: &HashMap<String, f64>) -> f64 {
    match expr {
        Expression::Number(n) => *n,

        Expression::Symbol(name) => vars.get(name.as_str()).copied().unwrap_or(f64::NAN),

        Expression::Keyword(_) => f64::NAN, // colors are not numeric

        Expression::List(items) if items.is_empty() => 0.0,

        Expression::List(items) => {
            let Some(Expression::Symbol(head)) = items.first() else {
                return f64::NAN;
            };
            let args = &items[1..];
            match head.as_str() {
                // Arithmetic
                "+" => {
                    let mut sum = eval_sdf_expr(&args[0], vars);
                    for arg in &args[1..] {
                        sum += eval_sdf_expr(arg, vars);
                    }
                    sum
                }
                "-" => {
                    if args.len() == 1 {
                        -eval_sdf_expr(&args[0], vars)
                    } else {
                        let mut result = eval_sdf_expr(&args[0], vars);
                        for arg in &args[1..] {
                            result -= eval_sdf_expr(arg, vars);
                        }
                        result
                    }
                }
                "*" => {
                    let mut product = eval_sdf_expr(&args[0], vars);
                    for arg in &args[1..] {
                        product *= eval_sdf_expr(arg, vars);
                    }
                    product
                }
                "/" => {
                    if args.len() == 2 {
                        eval_sdf_expr(&args[0], vars) / eval_sdf_expr(&args[1], vars)
                    } else {
                        f64::NAN
                    }
                }

                // Math intrinsics
                "abs" => eval_sdf_expr(&args[0], vars).abs(),
                "sqrt" => eval_sdf_expr(&args[0], vars).sqrt(),
                "sin" => eval_sdf_expr(&args[0], vars).sin(),
                "cos" => eval_sdf_expr(&args[0], vars).cos(),
                "floor" => eval_sdf_expr(&args[0], vars).floor(),
                "ceil" => eval_sdf_expr(&args[0], vars).ceil(),
                "fract" => eval_sdf_expr(&args[0], vars).fract(),

                "min" => {
                    let mut result = eval_sdf_expr(&args[0], vars);
                    for arg in &args[1..] {
                        result = result.min(eval_sdf_expr(arg, vars));
                    }
                    result
                }
                "max" => {
                    let mut result = eval_sdf_expr(&args[0], vars);
                    for arg in &args[1..] {
                        result = result.max(eval_sdf_expr(arg, vars));
                    }
                    result
                }
                "pow" => eval_sdf_expr(&args[0], vars).powf(eval_sdf_expr(&args[1], vars)),
                "atan2" => {
                    eval_sdf_expr(&args[0], vars).atan2(eval_sdf_expr(&args[1], vars))
                }
                "clamp" => {
                    let v = eval_sdf_expr(&args[0], vars);
                    let lo = eval_sdf_expr(&args[1], vars);
                    let hi = eval_sdf_expr(&args[2], vars);
                    v.clamp(lo, hi)
                }
                "mix" => {
                    let a = eval_sdf_expr(&args[0], vars);
                    let b = eval_sdf_expr(&args[1], vars);
                    let t = eval_sdf_expr(&args[2], vars);
                    a + (b - a) * t
                }
                "smoothstep" => {
                    let e0 = eval_sdf_expr(&args[0], vars);
                    let e1 = eval_sdf_expr(&args[1], vars);
                    let x = eval_sdf_expr(&args[2], vars);
                    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
                    t * t * (3.0 - 2.0 * t)
                }

                // Vector ops — vec2 returns magnitude-like value, length computes it
                "vec2" => {
                    let vx = eval_sdf_expr(&args[0], vars);
                    let vy = eval_sdf_expr(&args[1], vars);
                    // Store as a pseudo-value; length() will re-evaluate
                    // Actually, we encode vec2 result as sqrt(x²+y²) when called by length
                    // But vec2 alone doesn't make sense as scalar. We need a special case.
                    // The pattern is always (length (vec2 ...)), so handle it in "length".
                    (vx * vx + vy * vy).sqrt()
                }
                "length" => {
                    // (length (vec2 a b)) → sqrt(a² + b²)
                    if let Some(Expression::List(inner)) = args.first() {
                        if matches!(inner.first(), Some(Expression::Symbol(s)) if s == "vec2")
                            && inner.len() == 3
                        {
                            let vx = eval_sdf_expr(&inner[1], vars);
                            let vy = eval_sdf_expr(&inner[2], vars);
                            return (vx * vx + vy * vy).sqrt();
                        }
                    }
                    // Fallback: treat arg as scalar magnitude
                    eval_sdf_expr(&args[0], vars).abs()
                }
                "dot" => {
                    // (dot (vec2 a b) (vec2 c d)) → a*c + b*d
                    if let (Some(Expression::List(a)), Some(Expression::List(b))) =
                        (args.first(), args.get(1))
                    {
                        if a.len() == 3 && b.len() == 3 {
                            let ax = eval_sdf_expr(&a[1], vars);
                            let ay = eval_sdf_expr(&a[2], vars);
                            let bx = eval_sdf_expr(&b[1], vars);
                            let by = eval_sdf_expr(&b[2], vars);
                            return ax * bx + ay * by;
                        }
                    }
                    f64::NAN
                }

                // Special forms
                "let" => {
                    if args.len() < 2 {
                        return f64::NAN;
                    }
                    let Expression::List(bindings) = &args[0] else {
                        return f64::NAN;
                    };
                    let mut new_vars = vars.clone();
                    for binding in bindings {
                        let Expression::List(pair) = binding else {
                            continue;
                        };
                        if pair.len() == 2 {
                            if let Expression::Symbol(name) = &pair[0] {
                                let val = eval_sdf_expr(&pair[1], &new_vars);
                                new_vars.insert(name.clone(), val);
                            }
                        }
                    }
                    // Evaluate body expressions, return last
                    let mut result = 0.0;
                    for body_expr in &args[1..] {
                        result = eval_sdf_expr(body_expr, &new_vars);
                    }
                    result
                }

                "if" => {
                    if args.len() >= 3 {
                        let cond = eval_sdf_expr(&args[0], vars);
                        if cond != 0.0 && !cond.is_nan() {
                            eval_sdf_expr(&args[1], vars)
                        } else {
                            eval_sdf_expr(&args[2], vars)
                        }
                    } else {
                        f64::NAN
                    }
                }

                // Comparison operators (return 1.0 for true, 0.0 for false)
                "=" => {
                    let a = eval_sdf_expr(&args[0], vars);
                    let b = eval_sdf_expr(&args[1], vars);
                    if (a - b).abs() < 1e-10 { 1.0 } else { 0.0 }
                }
                "<" => {
                    if eval_sdf_expr(&args[0], vars) < eval_sdf_expr(&args[1], vars) {
                        1.0
                    } else {
                        0.0
                    }
                }
                ">" => {
                    if eval_sdf_expr(&args[0], vars) > eval_sdf_expr(&args[1], vars) {
                        1.0
                    } else {
                        0.0
                    }
                }

                // SDF compositing — for hit testing we only care about sdf/fill distances
                "sdf/fill" | "sdf/paint" | "sdf/stroke" => {
                    // Return the SDF distance of the first argument
                    eval_sdf_expr(&args[0], vars)
                }

                "sdf/layer" => {
                    // Not directly useful for distance — see sdf_hit_test below
                    f64::NAN
                }

                _ => f64::NAN,
            }
        }

        _ => f64::NAN,
    }
}

/// Extract all `sdf/fill` SDF expressions from an `sdf/layer` body.
/// Returns them in order (region 0, 1, 2, ...).
fn extract_fill_regions(expr: &Expression) -> Vec<&Expression> {
    let mut regions = Vec::new();
    if let Expression::List(items) = expr {
        if let Some(Expression::Symbol(head)) = items.first() {
            if head == "sdf/layer" {
                for child in &items[1..] {
                    if let Expression::List(child_items) = child {
                        if matches!(child_items.first(), Some(Expression::Symbol(s)) if s == "sdf/fill")
                        {
                            // The SDF distance expression is the first arg of sdf/fill
                            if child_items.len() >= 2 {
                                regions.push(&child_items[1]);
                            }
                        }
                    }
                }
            }
        }
    }
    regions
}

/// Hit-test an SDF widget at the given normalized coordinates.
///
/// `x` and `y` should be in the SDF coordinate space ([-1,1] with aspect correction).
/// Returns the index of the topmost `sdf/fill` region containing the point,
/// or -1 if no region is hit.
pub fn sdf_hit_test(sdf_expr: &Expression, x: f64, y: f64) -> i32 {
    let regions = extract_fill_regions(sdf_expr);
    if regions.is_empty() {
        return -1;
    }

    let mut vars = HashMap::new();
    vars.insert("x".to_string(), x);
    vars.insert("y".to_string(), y);
    // Hit testing variables — not pressed during hit test
    vars.insert("hit/hover".to_string(), 0.0);
    vars.insert("hit/active".to_string(), 0.0);
    vars.insert("hit/region".to_string(), -1.0);

    // Iterate top-to-bottom for early exit — topmost region wins
    for (i, region_sdf) in regions.iter().enumerate().rev() {
        let d = eval_sdf_expr(region_sdf, &vars);
        if d < 0.0 {
            return i as i32;
        }
    }
    -1
}

/// Convert widget-local layout coordinates to SDF normalized coordinates.
/// The SDF space uses [-1,1] on both axes with aspect correction matching
/// the shader's coordinate mapping.
pub fn layout_to_sdf_coords(
    local_col: f32,
    local_row: f32,
    rect_width: f32,
    rect_height: f32,
    pixel_aspect: f32,
) -> (f64, f64) {
    // Map [0, width] → [-1, 1] and [0, height] → [-1, 1]
    let u = local_col / rect_width;
    let v = local_row / rect_height;
    let mut x = (u * 2.0 - 1.0) as f64;
    let mut y = (v * 2.0 - 1.0) as f64;

    // Apply same aspect correction as shader
    let aspect = pixel_aspect as f64;
    x *= aspect.max(1.0);
    y *= (1.0 / aspect.max(0.0001)).max(1.0);

    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_expr(src: &str) -> Expression {
        let tokens = crate::parser::Parser::new(src.to_string()).parse().unwrap();
        let mut ast = crate::parser::ASTParser::new(tokens);
        ast.parse().unwrap().into_iter().next().unwrap()
    }

    fn expand_expr(src: &str) -> Expression {
        use crate::compiler::Compiler;
        use crate::runtime::Runtime;

        let rt = Runtime::new();
        let compiler = Compiler::new_repl(
            vec![], vec![], vec![],
            std::collections::HashSet::new(),
            HashMap::new(),
            HashMap::new(),
            0,
            rt.macros().clone(),
        );
        compiler.expand_macros(&parse_expr(src), 0)
    }

    #[test]
    fn eval_circle_at_origin() {
        let expr = expand_expr("(sdf/circle 0.5)");
        let mut vars = HashMap::new();
        vars.insert("x".to_string(), 0.0);
        vars.insert("y".to_string(), 0.0);
        let d = eval_sdf_expr(&expr, &vars);
        assert!((d - (-0.5)).abs() < 1e-10, "got {}", d);
    }

    #[test]
    fn eval_circle_outside() {
        let expr = expand_expr("(sdf/circle 0.5)");
        let mut vars = HashMap::new();
        vars.insert("x".to_string(), 1.0);
        vars.insert("y".to_string(), 0.0);
        let d = eval_sdf_expr(&expr, &vars);
        assert!((d - 0.5).abs() < 1e-10, "got {}", d);
    }

    #[test]
    fn eval_translated_circle() {
        let expr = expand_expr("(sdf/translate 1 0 (sdf/circle 0.5))");
        let mut vars = HashMap::new();
        vars.insert("x".to_string(), 1.0);
        vars.insert("y".to_string(), 0.0);
        let d = eval_sdf_expr(&expr, &vars);
        assert!((d - (-0.5)).abs() < 1e-10, "got {}", d);
    }

    #[test]
    fn eval_rect_at_origin() {
        let expr = expand_expr("(sdf/rect 2 1)");
        let mut vars = HashMap::new();
        vars.insert("x".to_string(), 0.0);
        vars.insert("y".to_string(), 0.0);
        let d = eval_sdf_expr(&expr, &vars);
        assert!((d - (-1.0)).abs() < 1e-10, "got {}", d);
    }

    #[test]
    fn hit_test_single_fill() {
        let expr = expand_expr(
            "(sdf/layer (sdf/fill (sdf/circle 0.5) :accent))",
        );
        // Inside the circle
        assert_eq!(sdf_hit_test(&expr, 0.0, 0.0), 0);
        // Outside
        assert_eq!(sdf_hit_test(&expr, 1.0, 0.0), -1);
    }

    #[test]
    fn hit_test_two_fills() {
        let expr = expand_expr(
            "(sdf/layer
               (sdf/fill (sdf/circle 0.8) :accent)
               (sdf/fill (sdf/circle 0.3) :primary))",
        );
        // Inside both — topmost (region 1) wins
        assert_eq!(sdf_hit_test(&expr, 0.0, 0.0), 1);
        // Inside only the large circle
        assert_eq!(sdf_hit_test(&expr, 0.5, 0.0), 0);
        // Outside both
        assert_eq!(sdf_hit_test(&expr, 1.0, 0.0), -1);
    }

    #[test]
    fn hit_test_paint_ignored() {
        let expr = expand_expr(
            "(sdf/layer
               (sdf/fill (sdf/circle 0.8) :accent)
               (sdf/paint (sdf/circle 0.3) :primary))",
        );
        // Only one sdf/fill, so max region is 0
        assert_eq!(sdf_hit_test(&expr, 0.0, 0.0), 0);
    }

    #[test]
    fn hit_test_translated_fill() {
        let expr = expand_expr(
            "(sdf/layer
               (sdf/fill (sdf/translate 0.5 0 (sdf/circle 0.3)) :accent))",
        );
        // At (0.5, 0) — inside the translated circle
        assert_eq!(sdf_hit_test(&expr, 0.5, 0.0), 0);
        // At origin — outside
        assert_eq!(sdf_hit_test(&expr, 0.0, 0.0), -1);
    }

    #[test]
    fn layout_to_sdf_center() {
        let (x, y) = layout_to_sdf_coords(5.0, 5.0, 10.0, 10.0, 1.0);
        assert!(x.abs() < 1e-10);
        assert!(y.abs() < 1e-10);
    }
}
