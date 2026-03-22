use crate::parser::Expression;
use crate::theme;
use crate::vm::Value;
use std::collections::HashMap;
use std::fmt::Write;

#[derive(Debug)]
pub enum CodegenError {
    UnsupportedExpression(String),
    UnknownFunction(String),
    UnknownThemeColor(String),
}

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedExpression(s) => write!(f, "unsupported expression: {}", s),
            Self::UnknownFunction(s) => write!(f, "unknown function: {}", s),
            Self::UnknownThemeColor(s) => write!(f, "unknown theme color: :{}", s),
        }
    }
}

struct MetalEmitter {
    next_var: usize,
    statements: Vec<String>,
    scopes: Vec<HashMap<String, String>>,
    current_region_id: Option<usize>,
    next_region_id: usize,
}

impl MetalEmitter {
    fn new() -> Self {
        Self {
            next_var: 0,
            statements: Vec::new(),
            scopes: Vec::new(),
            current_region_id: None,
            next_region_id: 0,
        }
    }

    fn fresh_var(&mut self) -> String {
        let name = format!("_v{}", self.next_var);
        self.next_var += 1;
        name
    }

    fn resolve_symbol(&self, name: &str) -> String {
        // Hit contextual variables
        match name {
            "hit/hover" => {
                return if let Some(rid) = self.current_region_id {
                    format!("(hit_region == {})", rid)
                } else {
                    "false".to_string()
                };
            }
            "hit/active" => {
                return if let Some(rid) = self.current_region_id {
                    format!("((hit_region == {}) && (hit_pressed != 0))", rid)
                } else {
                    "false".to_string()
                };
            }
            "hit/region" => return "hit_region".to_string(),
            _ => {}
        }

        for scope in self.scopes.iter().rev() {
            if let Some(metal_name) = scope.get(name) {
                return metal_name.clone();
            }
        }
        // Remap Lisp names to Metal-safe identifiers
        name.replace('-', "_")
    }

    fn emit_expr(&mut self, expr: &Expression) -> Result<String, CodegenError> {
        match expr {
            Expression::Number(n) => Ok(format_float(*n)),

            Expression::Symbol(name) => Ok(self.resolve_symbol(name)),

            Expression::Keyword(name) => {
                let color = theme::named_color(name)
                    .ok_or_else(|| CodegenError::UnknownThemeColor(name.clone()))?;
                Ok(format!(
                    "float4({}, {}, {}, {})",
                    format_float(color.r as f64),
                    format_float(color.g as f64),
                    format_float(color.b as f64),
                    format_float(color.a as f64),
                ))
            }

            Expression::List(items) if items.is_empty() => Ok("0.0".to_string()),

            Expression::List(items) => {
                let Some(Expression::Symbol(head)) = items.first() else {
                    return Err(CodegenError::UnsupportedExpression(
                        "list head must be a symbol".into(),
                    ));
                };
                let args = &items[1..];
                match head.as_str() {
                    // Special forms
                    "let" => self.emit_let(args),
                    "if" => self.emit_if(args),
                    "do" => self.emit_do(args),

                    // Arithmetic operators (variadic)
                    "+" | "-" | "*" | "/" => self.emit_arithmetic(head, args),

                    // Comparison operators
                    "=" => self.emit_binary_op("==", args),
                    "<" | ">" | "<=" | ">=" => self.emit_binary_op(head, args),

                    // min/max are variadic in Lisp, binary in Metal
                    "min" | "max" => self.emit_variadic_func(head, args),

                    // Vector constructors
                    "vec2" => self.emit_func_call("float2", args),
                    "vec3" => self.emit_func_call("float3", args),
                    "vec4" => self.emit_func_call("float4", args),

                    // 1-arg math intrinsics (same name in Metal)
                    "abs" | "sin" | "cos" | "sqrt" | "fract" | "floor" | "ceil" | "round"
                    | "length" => self.emit_func_call(head, args),

                    // 2-arg math intrinsics
                    "pow" | "atan2" | "dot" | "mod" => {
                        let metal_name = if head == "mod" { "fmod" } else { head };
                        self.emit_func_call(metal_name, args)
                    }

                    // 3-arg math intrinsics
                    "clamp" | "mix" | "smoothstep" => self.emit_func_call(head, args),

                    // SDF compositing forms (Milestone 2)
                    "sdf/layer" => self.emit_sdf_layer(args),
                    "sdf/fill" => self.emit_sdf_fill(args),
                    "sdf/paint" => self.emit_sdf_paint(args),
                    "sdf/stroke" => self.emit_sdf_stroke(args),

                    _ => Err(CodegenError::UnknownFunction(head.clone())),
                }
            }

            // Quasiquote/Unquote should be expanded before codegen
            Expression::Quasiquote(_) | Expression::Unquote(_) => Err(
                CodegenError::UnsupportedExpression("unexpanded quasiquote/unquote".into()),
            ),

            Expression::String(s) => Err(CodegenError::UnsupportedExpression(format!(
                "string literal: \"{}\"",
                s
            ))),

            Expression::QuoteSymbol(s) => Err(CodegenError::UnsupportedExpression(format!(
                "quoted symbol: '{}'",
                s
            ))),

            Expression::QuoteList(_) => {
                Err(CodegenError::UnsupportedExpression("quoted list".into()))
            }
        }
    }

    // ── SDF compositing forms ────────────────────────────────────────────

    fn emit_sdf_layer(&mut self, children: &[Expression]) -> Result<String, CodegenError> {
        let layer_var = self.fresh_var();
        self.statements
            .push(format!("float4 {} = float4(0.0, 0.0, 0.0, 0.0);", layer_var));

        for child in children {
            let child_color = self.emit_expr(child)?;
            let c = self.fresh_var();
            self.statements
                .push(format!("float4 {} = {};", c, child_color));
            self.statements
                .push(format!("{0} = {1} + {0} * (1.0 - {1}.a);", layer_var, c));
        }

        Ok(layer_var)
    }

    /// Shared logic for sdf/fill and sdf/paint.
    /// If `is_fill` is true, assigns a hit region and sets contextual variables.
    fn emit_sdf_shape_color(
        &mut self,
        sdf_expr: &Expression,
        color_expr: &Expression,
        is_fill: bool,
    ) -> Result<String, CodegenError> {
        let region_id = if is_fill {
            let rid = self.next_region_id;
            self.next_region_id += 1;
            self.current_region_id = Some(rid);
            Some(rid)
        } else {
            None
        };

        // Evaluate the SDF distance
        let dist = self.emit_expr(sdf_expr)?;
        let d = self.fresh_var();
        self.statements.push(format!("float {} = {};", d, dist));

        // AA mask
        let aa = self.fresh_var();
        self.statements
            .push(format!("float {} = max(fwidth({}), 0.001);", aa, d));
        let mask = self.fresh_var();
        self.statements
            .push(format!("float {} = smoothstep({}, -({}), {});", mask, aa, aa, d));

        // Evaluate color (may reference hit/hover, hit/active)
        let color = self.emit_expr(color_expr)?;
        let clr = self.fresh_var();
        self.statements.push(format!("float4 {} = {};", clr, color));

        // Premultiplied alpha output
        let result = self.fresh_var();
        self.statements.push(format!(
            "float4 {} = float4({}.rgb * {}.a * {}, {}.a * {});",
            result, clr, clr, mask, clr, mask
        ));

        // Keep region context active for subsequent sdf/paint siblings
        let _ = region_id;

        Ok(result)
    }

    fn emit_sdf_fill(&mut self, args: &[Expression]) -> Result<String, CodegenError> {
        if args.len() < 2 {
            return Err(CodegenError::UnsupportedExpression(
                "sdf/fill requires (sdf-expr color-expr)".into(),
            ));
        }
        self.emit_sdf_shape_color(&args[0], &args[1], true)
    }

    fn emit_sdf_paint(&mut self, args: &[Expression]) -> Result<String, CodegenError> {
        if args.len() < 2 {
            return Err(CodegenError::UnsupportedExpression(
                "sdf/paint requires (sdf-expr color-expr)".into(),
            ));
        }
        self.emit_sdf_shape_color(&args[0], &args[1], false)
    }

    fn emit_sdf_stroke(&mut self, args: &[Expression]) -> Result<String, CodegenError> {
        if args.len() < 3 {
            return Err(CodegenError::UnsupportedExpression(
                "sdf/stroke requires (sdf-expr width color-expr)".into(),
            ));
        }

        // Evaluate the SDF distance
        let dist = self.emit_expr(&args[0])?;
        let d = self.fresh_var();
        self.statements.push(format!("float {} = {};", d, dist));

        // Convert to stroke: abs(d) - width
        let width = self.emit_expr(&args[1])?;
        let stroke_d = self.fresh_var();
        self.statements
            .push(format!("float {} = abs({}) - {};", stroke_d, d, width));

        // AA mask
        let aa = self.fresh_var();
        self.statements
            .push(format!("float {} = max(fwidth({}), 0.001);", aa, stroke_d));
        let mask = self.fresh_var();
        self.statements
            .push(format!("float {} = smoothstep({}, -({}), {});", mask, aa, aa, stroke_d));

        // Evaluate color
        let color = self.emit_expr(&args[2])?;
        let clr = self.fresh_var();
        self.statements.push(format!("float4 {} = {};", clr, color));

        // Premultiplied alpha output
        let result = self.fresh_var();
        self.statements.push(format!(
            "float4 {} = float4({}.rgb * {}.a * {}, {}.a * {});",
            result, clr, clr, mask, clr, mask
        ));

        Ok(result)
    }

    fn emit_let(&mut self, args: &[Expression]) -> Result<String, CodegenError> {
        if args.len() < 2 {
            return Err(CodegenError::UnsupportedExpression("let needs bindings and body".into()));
        }
        let Expression::List(bindings) = &args[0] else {
            return Err(CodegenError::UnsupportedExpression("let bindings must be a list".into()));
        };

        self.scopes.push(HashMap::new());
        for binding in bindings {
            let Expression::List(pair) = binding else {
                return Err(CodegenError::UnsupportedExpression(
                    "let binding must be a pair".into(),
                ));
            };
            if pair.len() != 2 {
                return Err(CodegenError::UnsupportedExpression(
                    "let binding must have exactly 2 elements".into(),
                ));
            }
            let Expression::Symbol(name) = &pair[0] else {
                return Err(CodegenError::UnsupportedExpression(
                    "let binding name must be a symbol".into(),
                ));
            };

            // Emit value before inserting binding (sequential let: sees prior bindings only)
            let val = self.emit_expr(&pair[1])?;
            let var = self.fresh_var();
            let type_name = metal_type_for_expr(&pair[1]).unwrap_or("float");
            self.statements
                .push(format!("{} {} = {};", type_name, var, val));
            self.scopes.last_mut().unwrap().insert(name.clone(), var);
        }

        let body = self.emit_body(&args[1..])?;
        self.scopes.pop();
        Ok(body)
    }

    fn emit_if(&mut self, args: &[Expression]) -> Result<String, CodegenError> {
        if args.len() < 3 {
            return Err(CodegenError::UnsupportedExpression("if needs 3 args".into()));
        }
        let cond = self.emit_expr(&args[0])?;
        let then = self.emit_expr(&args[1])?;
        let else_ = self.emit_expr(&args[2])?;
        Ok(format!("(({}) ? ({}) : ({}))", cond, then, else_))
    }

    fn emit_do(&mut self, args: &[Expression]) -> Result<String, CodegenError> {
        self.emit_body(args)
    }

    fn emit_body(&mut self, exprs: &[Expression]) -> Result<String, CodegenError> {
        if exprs.is_empty() {
            return Ok("0.0".to_string());
        }
        // Emit all but last as statements (side effects only in let bindings)
        // Last expression is the return value
        let mut result = String::new();
        for expr in exprs {
            result = self.emit_expr(expr)?;
        }
        Ok(result)
    }

    fn emit_arithmetic(
        &mut self,
        op: &str,
        args: &[Expression],
    ) -> Result<String, CodegenError> {
        if args.is_empty() {
            return Ok("0.0".to_string());
        }
        if args.len() == 1 && op == "-" {
            let a = self.emit_expr(&args[0])?;
            return Ok(format!("(-({}))", a));
        }
        let first = self.emit_expr(&args[0])?;
        if args.len() == 1 {
            return Ok(first);
        }
        let mut result = first;
        for arg in &args[1..] {
            let rhs = self.emit_expr(arg)?;
            result = format!("({} {} {})", result, op, rhs);
        }
        Ok(result)
    }

    fn emit_binary_op(&mut self, op: &str, args: &[Expression]) -> Result<String, CodegenError> {
        if args.len() != 2 {
            return Err(CodegenError::UnsupportedExpression(format!(
                "{} requires 2 args",
                op
            )));
        }
        let a = self.emit_expr(&args[0])?;
        let b = self.emit_expr(&args[1])?;
        Ok(format!("({} {} {})", a, op, b))
    }

    fn emit_variadic_func(
        &mut self,
        name: &str,
        args: &[Expression],
    ) -> Result<String, CodegenError> {
        if args.len() < 2 {
            if args.len() == 1 {
                return self.emit_expr(&args[0]);
            }
            return Err(CodegenError::UnsupportedExpression(format!(
                "{} needs at least 1 arg",
                name
            )));
        }
        let first = self.emit_expr(&args[0])?;
        let mut result = first;
        for arg in &args[1..] {
            let rhs = self.emit_expr(arg)?;
            result = format!("{}({}, {})", name, result, rhs);
        }
        Ok(result)
    }

    fn emit_func_call(
        &mut self,
        metal_name: &str,
        args: &[Expression],
    ) -> Result<String, CodegenError> {
        let mut parts = Vec::new();
        for arg in args {
            parts.push(self.emit_expr(arg)?);
        }
        Ok(format!("{}({})", metal_name, parts.join(", ")))
    }
}

/// Format an f64 as a Metal float literal.
fn format_float(n: f64) -> String {
    if n == n.trunc() && n.abs() < 1e15 {
        // Integer-valued: emit as "1.0" not "1"
        format!("{:.1}", n)
    } else {
        // Use enough precision without trailing zeros
        let s = format!("{}", n);
        if s.contains('.') || s.contains('e') || s.contains('E') {
            s
        } else {
            format!("{}.0", s)
        }
    }
}

/// Returns the Metal type if the expression produces a non-float type.
fn metal_type_for_expr(expr: &Expression) -> Option<&'static str> {
    if let Expression::List(items) = expr {
        if let Some(Expression::Symbol(s)) = items.first() {
            return match s.as_str() {
                "vec2" => Some("float2"),
                "vec3" => Some("float3"),
                "vec4" => Some("float4"),
                "sdf/fill" | "sdf/paint" | "sdf/stroke" | "sdf/layer" => Some("float4"),
                _ => None,
            };
        }
    }
    None
}

/// Convert a runtime Value (from a quoted form) back into an Expression for codegen.
pub fn value_to_expression(val: &Value) -> Result<Expression, CodegenError> {
    match val {
        Value::Number(n) => Ok(Expression::Number(*n)),
        Value::Symbol(s) => Ok(Expression::Symbol(s.clone())),
        Value::Keyword(s) => Ok(Expression::Keyword(s.clone())),
        Value::String(s) => Ok(Expression::String(s.clone())),
        Value::List(items) => {
            let exprs: Result<Vec<_>, _> = items
                .iter()
                .map(|item| value_to_expression(&item.borrow()))
                .collect();
            Ok(Expression::List(exprs?))
        }
        Value::Bool(true) => Ok(Expression::Symbol("true".into())),
        Value::Bool(false) => Ok(Expression::Symbol("false".into())),
        Value::Nil => Ok(Expression::Symbol("nil".into())),
        _ => Err(CodegenError::UnsupportedExpression(format!(
            "cannot convert {:?} to expression",
            val
        ))),
    }
}

/// Result of SDF shader compilation.
pub struct SdfShaderOutput {
    pub shader_source: String,
    pub region_count: usize,
}

/// Compile a macro-expanded SDF expression into a complete Metal fragment shader.
///
/// Supports both single-SDF expressions (returns distance-based rendering)
/// and `sdf/layer` expressions (returns composited multi-shape rendering with hit regions).
pub fn compile_sdf_to_metal(expr: &Expression) -> Result<SdfShaderOutput, CodegenError> {
    let is_layer = matches!(expr, Expression::List(items)
        if matches!(items.first(), Some(Expression::Symbol(s)) if s == "sdf/layer"));

    let mut emitter = MetalEmitter::new();
    let result_expr = emitter.emit_expr(expr)?;
    let region_count = emitter.next_region_id;

    let mut shader = String::with_capacity(2048);
    writeln!(shader, "fragment float4 widget_frag(WidgetVaryings in [[stage_in]])").unwrap();
    writeln!(shader, "{{").unwrap();
    writeln!(shader, "    float aspect = in.aspect;").unwrap();
    writeln!(shader, "    float x = (in.uv.x * 2.0 - 1.0) * max(aspect, 1.0);").unwrap();
    writeln!(shader, "    float y = (in.uv.y * 2.0 - 1.0) * max(1.0 / max(aspect, 0.0001), 1.0);").unwrap();
    writeln!(shader, "    float value_t = in.value_t;").unwrap();

    if is_layer {
        // Hit region uniforms packed into color_b
        writeln!(shader, "    int hit_region = int(in.color_b.x);").unwrap();
        writeln!(shader, "    int hit_pressed = int(in.color_b.y);").unwrap();
    }

    for stmt in &emitter.statements {
        writeln!(shader, "    {}", stmt).unwrap();
    }

    if is_layer {
        writeln!(shader, "    float4 result = {};", result_expr).unwrap();
        writeln!(shader, "    if (result.a < 0.001) discard_fragment();").unwrap();
        writeln!(shader, "    return result;").unwrap();
    } else {
        writeln!(shader, "    float d = {};", result_expr).unwrap();
        writeln!(shader, "    float aa = max(fwidth(d), 0.001);").unwrap();
        writeln!(shader, "    float mask = smoothstep(aa, -aa, d);").unwrap();
        writeln!(shader, "    if (mask < 0.001) discard_fragment();").unwrap();
        writeln!(shader, "    float4 fill_color = in.color_a;").unwrap();
        writeln!(shader, "    return float4(fill_color.rgb, fill_color.a * mask);").unwrap();
    }
    writeln!(shader, "}}").unwrap();

    Ok(SdfShaderOutput {
        shader_source: shader,
        region_count,
    })
}

/// Compile only the SDF expression (no shader wrapper). Useful for testing
/// and for Milestone 2 where sdf/layer composes multiple SDF expressions.
pub fn compile_sdf_expr(expr: &Expression) -> Result<(Vec<String>, String), CodegenError> {
    let mut emitter = MetalEmitter::new();
    let result = emitter.emit_expr(expr)?;
    Ok((emitter.statements, result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Expression;

    fn parse_one_expr(src: &str) -> Expression {
        let tokens = crate::parser::Parser::new(src.to_string()).parse().unwrap();
        let mut ast = crate::parser::ASTParser::new(tokens);
        ast.parse().unwrap().into_iter().next().unwrap()
    }

    fn codegen_expr(src: &str) -> (Vec<String>, String) {
        compile_sdf_expr(&parse_one_expr(src)).unwrap()
    }

    #[test]
    fn number_literal() {
        let (stmts, result) = codegen_expr("0.5");
        assert!(stmts.is_empty());
        assert_eq!(result, "0.5");
    }

    #[test]
    fn integer_as_float() {
        let (_, result) = codegen_expr("42");
        assert_eq!(result, "42.0");
    }

    #[test]
    fn symbol_passthrough() {
        let (_, result) = codegen_expr("x");
        assert_eq!(result, "x");
    }

    #[test]
    fn simple_subtraction() {
        let (_, result) = codegen_expr("(- x 0.5)");
        assert_eq!(result, "(x - 0.5)");
    }

    #[test]
    fn nested_arithmetic() {
        let (_, result) = codegen_expr("(+ (* x 2) y)");
        assert_eq!(result, "((x * 2.0) + y)");
    }

    #[test]
    fn unary_negation() {
        let (_, result) = codegen_expr("(- x)");
        assert_eq!(result, "(-(x))");
    }

    #[test]
    fn vec2_constructor() {
        let (_, result) = codegen_expr("(vec2 x y)");
        assert_eq!(result, "float2(x, y)");
    }

    #[test]
    fn length_of_vec2() {
        let (_, result) = codegen_expr("(length (vec2 x y))");
        assert_eq!(result, "length(float2(x, y))");
    }

    #[test]
    fn circle_sdf_expanded() {
        // The expanded form of (sdf/circle 0.5)
        let (stmts, result) = codegen_expr("(- (length (vec2 x y)) 0.5)");
        assert!(stmts.is_empty());
        assert_eq!(result, "(length(float2(x, y)) - 0.5)");
    }

    #[test]
    fn let_binding() {
        let (stmts, result) = codegen_expr("(let ((dx (- x 0.5))) dx)");
        assert_eq!(stmts.len(), 1);
        assert!(stmts[0].contains("_v0"));
        assert!(stmts[0].contains("(x - 0.5)"));
        assert_eq!(result, "_v0");
    }

    #[test]
    fn let_with_shadowing() {
        // Simulates what sdf/translate expands to
        let (stmts, result) =
            codegen_expr("(let ((x (- x 0.5)) (y (- y 0.3))) (length (vec2 x y)))");
        assert_eq!(stmts.len(), 2);
        // First binding uses original x
        assert!(stmts[0].contains("(x - 0.5)"));
        // Second binding uses original y (sequential let, but y hasn't been rebound yet)
        assert!(stmts[1].contains("(y - 0.3)"));
        // Body uses the shadowed x and y
        assert!(result.contains("_v0"));
        assert!(result.contains("_v1"));
    }

    #[test]
    fn if_expression() {
        let (_, result) = codegen_expr("(if (< x 0) 1 0)");
        assert!(result.contains("?"));
        assert!(result.contains("(x < 0.0)"));
    }

    #[test]
    fn min_max_variadic() {
        let (_, result) = codegen_expr("(min (max 0 x) 1)");
        assert_eq!(result, "min(max(0.0, x), 1.0)");
    }

    #[test]
    fn math_intrinsics() {
        let (_, result) = codegen_expr("(smoothstep 0 1 x)");
        assert_eq!(result, "smoothstep(0.0, 1.0, x)");
    }

    #[test]
    fn clamp_call() {
        let (_, result) = codegen_expr("(clamp x 0 1)");
        assert_eq!(result, "clamp(x, 0.0, 1.0)");
    }

    #[test]
    fn full_shader_has_structure() {
        let expr = parse_one_expr("(- (length (vec2 x y)) 0.5)");
        let output = compile_sdf_to_metal(&expr).unwrap();
        let shader = &output.shader_source;
        assert!(shader.contains("fragment float4 widget_frag"));
        assert!(shader.contains("float x ="));
        assert!(shader.contains("float y ="));
        assert!(shader.contains("float d ="));
        assert!(shader.contains("fwidth"));
        assert!(shader.contains("smoothstep"));
        assert!(shader.contains("discard_fragment"));
        assert_eq!(output.region_count, 0);
    }

    #[test]
    fn rect_sdf_expanded() {
        // Manually construct the expanded form of sdf/rect
        let (stmts, result) = codegen_expr(
            "(let ((dx (- (abs x) 2))
                  (dy (- (abs y) 1)))
               (+ (length (vec2 (max dx 0) (max dy 0)))
                  (min (max dx dy) 0)))",
        );
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].contains("abs(x)"));
        assert!(result.contains("length"));
        assert!(result.contains("min"));
    }

    #[test]
    fn theme_color_keyword() {
        let expr = parse_one_expr(":accent");
        let result = compile_sdf_expr(&expr);
        assert!(result.is_ok());
        let (_, s) = result.unwrap();
        assert!(s.starts_with("float4("));
    }

    #[test]
    fn mod_becomes_fmod() {
        let (_, result) = codegen_expr("(mod x 1)");
        assert_eq!(result, "fmod(x, 1.0)");
    }

    #[test]
    fn hyphenated_symbols_become_underscores() {
        let (_, result) = codegen_expr("my-var");
        assert_eq!(result, "my_var");
    }

    // ── Integration tests: SDF macro → expand → codegen ─────────────────

    /// Expand SDF macros and return the expanded Expression.
    fn expand_sdf(src: &str) -> Expression {
        use crate::compiler::Compiler;
        use crate::runtime::Runtime;

        let rt = Runtime::new();
        let compiler = Compiler::new_repl(
            vec![], vec![], vec![],
            std::collections::HashSet::new(),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            0,
            rt.macros().clone(),
        );
        compiler.expand_macros(&parse_one_expr(src), 0)
    }

    fn macro_to_metal(src: &str) -> (Vec<String>, String) {
        compile_sdf_expr(&expand_sdf(src)).unwrap()
    }

    #[test]
    fn e2e_sdf_circle_codegen() {
        let (stmts, result) = macro_to_metal("(sdf/circle 0.5)");
        assert!(stmts.is_empty());
        assert_eq!(result, "(length(float2(x, y)) - 0.5)");
    }

    #[test]
    fn e2e_sdf_rect_codegen() {
        let (stmts, result) = macro_to_metal("(sdf/rect 2 1)");
        assert_eq!(stmts.len(), 2);
        // dx = abs(x) - 2, dy = abs(y) - 1
        assert!(stmts[0].contains("abs(x)"));
        assert!(stmts[1].contains("abs(y)"));
        // body uses length, min, max
        assert!(result.contains("length"));
    }

    #[test]
    fn e2e_sdf_translate_circle_codegen() {
        let (stmts, result) = macro_to_metal("(sdf/translate 0.5 0.3 (sdf/circle 0.2))");
        // translate rebinds x, y; circle uses them
        assert!(stmts.len() >= 2);
        // The body should reference the rebound variables
        assert!(result.contains("length"));
    }

    #[test]
    fn e2e_sdf_union_codegen() {
        let (_, result) = macro_to_metal("(sdf/union (sdf/circle 1) (sdf/circle 0.5))");
        assert!(result.starts_with("min("));
    }

    #[test]
    fn e2e_full_shader_from_macro() {
        let output = compile_sdf_to_metal(&expand_sdf("(sdf/circle 0.5)")).unwrap();
        assert!(output.shader_source.contains("fragment float4 widget_frag"));
        assert!(output.shader_source.contains("length(float2(x, y)) - 0.5"));
        assert!(output.shader_source.contains("discard_fragment"));
    }

    // ── Milestone 2: sdf/layer, sdf/fill, sdf/paint, sdf/stroke ────────

    #[test]
    fn sdf_fill_basic() {
        let (stmts, result) = codegen_expr("(sdf/fill (- (length (vec2 x y)) 0.5) :accent)");
        // Should produce AA masking and premultiplied color output
        assert!(stmts.iter().any(|s| s.contains("smoothstep")));
        assert!(stmts.iter().any(|s| s.contains("fwidth")));
        assert!(stmts.iter().any(|s| s.contains("float4")));
        // Result should be a float4 variable
        assert!(result.starts_with("_v"));
    }

    #[test]
    fn sdf_paint_basic() {
        let (stmts, result) = codegen_expr("(sdf/paint (- (length (vec2 x y)) 0.3) :primary)");
        assert!(stmts.iter().any(|s| s.contains("smoothstep")));
        assert!(result.starts_with("_v"));
    }

    #[test]
    fn sdf_stroke_basic() {
        let (stmts, _result) = codegen_expr("(sdf/stroke (- (length (vec2 x y)) 0.5) 0.02 :accent)");
        // Stroke converts distance: abs(d) - width
        assert!(stmts.iter().any(|s| s.contains("abs(")));
        assert!(stmts.iter().any(|s| s.contains("0.02")));
    }

    #[test]
    fn sdf_fill_assigns_region() {
        let mut emitter = MetalEmitter::new();
        let expr = parse_one_expr("(sdf/fill (- (length (vec2 x y)) 0.5) :accent)");
        let _ = emitter.emit_expr(&expr).unwrap();
        assert_eq!(emitter.next_region_id, 1);
    }

    #[test]
    fn sdf_paint_no_region() {
        let mut emitter = MetalEmitter::new();
        let expr = parse_one_expr("(sdf/paint (- (length (vec2 x y)) 0.5) :accent)");
        let _ = emitter.emit_expr(&expr).unwrap();
        assert_eq!(emitter.next_region_id, 0);
    }

    #[test]
    fn sdf_fill_hit_hover() {
        let (stmts, _) = codegen_expr(
            "(sdf/fill (- (length (vec2 x y)) 0.5) (if hit/hover :accent :primary))",
        );
        // hit/hover should resolve to (hit_region == 0)
        let all = stmts.join("\n");
        assert!(all.contains("hit_region == 0"));
    }

    #[test]
    fn sdf_layer_single_fill() {
        let (stmts, result) = codegen_expr(
            "(sdf/layer (sdf/fill (- (length (vec2 x y)) 0.5) :accent))",
        );
        // Should have layer accumulator and alpha blend
        let all = stmts.join("\n");
        assert!(all.contains("float4"));
        assert!(all.contains("1.0 -"));
        assert!(result.starts_with("_v"));
    }

    #[test]
    fn sdf_layer_two_fills_get_different_regions() {
        let mut emitter = MetalEmitter::new();
        let expr = parse_one_expr(
            "(sdf/layer
               (sdf/fill (- (length (vec2 x y)) 0.5) :accent)
               (sdf/fill (- (length (vec2 x y)) 0.3) :primary))",
        );
        let _ = emitter.emit_expr(&expr).unwrap();
        assert_eq!(emitter.next_region_id, 2);
    }

    #[test]
    fn sdf_layer_fill_then_paint_shares_region_context() {
        let (stmts, _) = codegen_expr(
            "(sdf/layer
               (sdf/fill (- (length (vec2 x y)) 0.8) :accent)
               (sdf/paint (- (length (vec2 x y)) 0.2) (if hit/active :accent :primary)))",
        );
        let all = stmts.join("\n");
        // The paint's hit/active should reference region 0 (from the preceding fill)
        assert!(all.contains("hit_region == 0"));
        assert!(all.contains("hit_pressed"));
    }

    #[test]
    fn sdf_layer_full_shader() {
        let output = compile_sdf_to_metal(&parse_one_expr(
            "(sdf/layer
               (sdf/fill (- (length (vec2 x y)) 0.8) :accent)
               (sdf/paint (- (length (vec2 x y)) 0.2) :primary))",
        ))
        .unwrap();
        let shader = &output.shader_source;
        assert!(shader.contains("int hit_region"));
        assert!(shader.contains("int hit_pressed"));
        assert!(shader.contains("discard_fragment"));
        assert_eq!(output.region_count, 1);
    }

    #[test]
    fn e2e_layer_with_macros() {
        let output = compile_sdf_to_metal(&expand_sdf(
            "(sdf/layer
               (sdf/fill (sdf/circle 0.8) :accent)
               (sdf/paint (sdf/circle 0.2) :primary))",
        ))
        .unwrap();
        assert!(output.shader_source.contains("fragment float4 widget_frag"));
        assert_eq!(output.region_count, 1);
    }
}
