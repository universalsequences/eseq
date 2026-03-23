use crate::parser::Expression;
use crate::theme;
use crate::vm::Value;
use std::collections::{HashMap, HashSet};
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
    uniform_symbols: HashMap<String, String>,
    current_region_id: Option<usize>,
    next_region_id: usize,
}

struct MaterialSpec<'a> {
    color_expr: &'a Expression,
    shadow: Option<ShadowSpec<'a>>,
}

struct ShadowSpec<'a> {
    color_expr: &'a Expression,
    blur_expr: &'a Expression,
    offset_expr: Option<&'a Expression>,
    spread_expr: Option<&'a Expression>,
}

impl MetalEmitter {
    fn new(uniform_symbols: HashMap<String, String>) -> Self {
        Self {
            next_var: 0,
            statements: Vec::new(),
            scopes: Vec::new(),
            uniform_symbols,
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
            "itime" => return "itime".to_string(),
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
        if let Some(uniform_name) = self.uniform_symbols.get(name) {
            return uniform_name.clone();
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
                    "rgba" => self.emit_func_call("float4", args),

                    // 1-arg math intrinsics (same name in Metal)
                    "abs" | "sin" | "cos" | "sqrt" | "fract" | "floor" | "ceil" | "round"
                    | "length" => self.emit_func_call(head, args),

                    // 2-arg math intrinsics
                    "pow" | "atan2" | "dot" | "mod" => {
                        let metal_name = if head == "mod" { "fmod" } else { head };
                        self.emit_func_call(metal_name, args)
                    }

                    // 3-arg math intrinsics
                    "clamp" | "smoothstep" => self.emit_func_call(head, args),
                    "mix" => self.emit_mix(args),

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
        self.statements.push(format!(
            "float4 {} = float4(0.0, 0.0, 0.0, 0.0);",
            layer_var
        ));

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
        material_expr: &Expression,
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
        self.statements.push(format!(
            "float {} = smoothstep({}, -({}), {});",
            mask, aa, aa, d
        ));

        let material = self.parse_material(material_expr)?;
        self.scopes
            .push(HashMap::from([("d".to_string(), d.clone())]));
        let shadow = self.emit_shadow_contribution(sdf_expr, &material, &d)?;
        let color = self.emit_expr(material.color_expr)?;
        self.scopes.pop();
        let clr = self.fresh_var();
        self.statements.push(format!("float4 {} = {};", clr, color));

        let fill_result = self.fresh_var();
        self.statements.push(format!(
            "float4 {} = float4({}.rgb * {}.a * {}, {}.a * {});",
            fill_result, clr, clr, mask, clr, mask
        ));

        let result = self.fresh_var();
        if let Some(shadow) = shadow {
            self.statements.push(format!(
                "float4 {} = {} + {} * (1.0 - {}.a);",
                result, fill_result, shadow, fill_result
            ));
        } else {
            self.statements
                .push(format!("float4 {} = {};", result, fill_result));
        }

        // Keep region context active for subsequent sdf/paint siblings
        let _ = region_id;

        Ok(result)
    }

    fn parse_material<'a>(&self, expr: &'a Expression) -> Result<MaterialSpec<'a>, CodegenError> {
        let Expression::List(items) = expr else {
            return Ok(MaterialSpec {
                color_expr: expr,
                shadow: None,
            });
        };
        let Some(Expression::Symbol(head)) = items.first() else {
            return Ok(MaterialSpec {
                color_expr: expr,
                shadow: None,
            });
        };
        if head != "material" {
            return Ok(MaterialSpec {
                color_expr: expr,
                shadow: None,
            });
        }

        let mut color_expr = None;
        let mut shadow_expr = None;
        let mut i = 1;
        while i + 1 < items.len() {
            if let Expression::Keyword(key) = &items[i] {
                match key.as_str() {
                    "color" => color_expr = Some(&items[i + 1]),
                    "shadow" => shadow_expr = Some(&items[i + 1]),
                    _ => {}
                }
                i += 2;
            } else {
                i += 1;
            }
        }

        let Some(color_expr) = color_expr else {
            return Err(CodegenError::UnsupportedExpression(
                "material requires :color".into(),
            ));
        };

        Ok(MaterialSpec {
            color_expr,
            shadow: shadow_expr.map(Self::parse_shadow).transpose()?,
        })
    }

    fn parse_shadow<'a>(expr: &'a Expression) -> Result<ShadowSpec<'a>, CodegenError> {
        let Expression::List(items) = expr else {
            return Err(CodegenError::UnsupportedExpression(
                "shadow must be a form".into(),
            ));
        };
        let Some(Expression::Symbol(head)) = items.first() else {
            return Err(CodegenError::UnsupportedExpression(
                "shadow head must be a symbol".into(),
            ));
        };
        if head != "shadow" {
            return Err(CodegenError::UnsupportedExpression(
                "material :shadow must use shadow".into(),
            ));
        }

        let mut color_expr = None;
        let mut blur_expr = None;
        let mut offset_expr = None;
        let mut spread_expr = None;
        let mut i = 1;
        while i + 1 < items.len() {
            if let Expression::Keyword(key) = &items[i] {
                match key.as_str() {
                    "color" => color_expr = Some(&items[i + 1]),
                    "blur" => blur_expr = Some(&items[i + 1]),
                    "offset" => offset_expr = Some(&items[i + 1]),
                    "spread" => spread_expr = Some(&items[i + 1]),
                    _ => {}
                }
                i += 2;
            } else {
                i += 1;
            }
        }

        let Some(color_expr) = color_expr else {
            return Err(CodegenError::UnsupportedExpression(
                "shadow requires :color".into(),
            ));
        };
        let Some(blur_expr) = blur_expr else {
            return Err(CodegenError::UnsupportedExpression(
                "shadow requires :blur".into(),
            ));
        };

        Ok(ShadowSpec {
            color_expr,
            blur_expr,
            offset_expr,
            spread_expr,
        })
    }

    fn emit_shadow_contribution(
        &mut self,
        sdf_expr: &Expression,
        material: &MaterialSpec<'_>,
        d: &str,
    ) -> Result<Option<String>, CodegenError> {
        let Some(shadow) = &material.shadow else {
            return Ok(None);
        };

        let default_offset_expr = Expression::List(vec![
            Expression::Symbol("vec2".into()),
            Expression::Number(0.0),
            Expression::Number(0.0),
        ]);
        let offset = match shadow.offset_expr {
            Some(expr) => self.emit_expr(expr)?,
            None => self.emit_expr(&default_offset_expr)?,
        };
        let offset_var = self.fresh_var();
        self.statements
            .push(format!("float2 {} = {};", offset_var, offset));

        let shadow_x = self.fresh_var();
        self.statements
            .push(format!("float {} = x - {}.x;", shadow_x, offset_var));
        let shadow_y = self.fresh_var();
        self.statements
            .push(format!("float {} = y - {}.y;", shadow_y, offset_var));

        self.scopes.push(HashMap::from([
            ("x".to_string(), shadow_x.clone()),
            ("y".to_string(), shadow_y.clone()),
            ("d".to_string(), d.to_string()),
        ]));
        let shadow_dist_expr = self.emit_expr(sdf_expr)?;
        let shadow_color_expr = self.emit_expr(shadow.color_expr)?;
        let blur_expr = self.emit_expr(shadow.blur_expr)?;
        let spread_expr = match shadow.spread_expr {
            Some(expr) => self.emit_expr(expr)?,
            None => "0.0".to_string(),
        };
        self.scopes.pop();

        let shadow_d = self.fresh_var();
        self.statements.push(format!(
            "float {} = ({} - {});",
            shadow_d, shadow_dist_expr, spread_expr
        ));
        let shadow_soft = self.fresh_var();
        self.statements.push(format!(
            "float {} = max(max({}, fwidth({})), 0.001);",
            shadow_soft, blur_expr, shadow_d
        ));
        let shadow_mask = self.fresh_var();
        self.statements.push(format!(
            "float {} = smoothstep({}, -({}), {});",
            shadow_mask, shadow_soft, shadow_soft, shadow_d
        ));
        let shadow_color = self.fresh_var();
        self.statements
            .push(format!("float4 {} = {};", shadow_color, shadow_color_expr));
        let shadow_result = self.fresh_var();
        self.statements.push(format!(
            "float4 {} = float4({}.rgb * {}.a * {}, {}.a * {});",
            shadow_result, shadow_color, shadow_color, shadow_mask, shadow_color, shadow_mask
        ));
        Ok(Some(shadow_result))
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
        self.statements.push(format!(
            "float {} = smoothstep({}, -({}), {});",
            mask, aa, aa, stroke_d
        ));

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
            return Err(CodegenError::UnsupportedExpression(
                "let needs bindings and body".into(),
            ));
        }
        let Expression::List(bindings) = &args[0] else {
            return Err(CodegenError::UnsupportedExpression(
                "let bindings must be a list".into(),
            ));
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
            return Err(CodegenError::UnsupportedExpression(
                "if needs 3 args".into(),
            ));
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

    fn emit_arithmetic(&mut self, op: &str, args: &[Expression]) -> Result<String, CodegenError> {
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

    fn emit_mix(&mut self, args: &[Expression]) -> Result<String, CodegenError> {
        if args.len() != 3 {
            return Err(CodegenError::UnsupportedExpression(
                "mix requires 3 args".into(),
            ));
        }
        let a = self.emit_expr(&args[0])?;
        let b = self.emit_expr(&args[1])?;
        let t = self.emit_expr(&args[2])?;
        Ok(format!("(({}) + ((({}) - ({})) * ({})))", a, b, a, t))
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
                "rgba" => Some("float4"),
                "sdf/fill" | "sdf/paint" | "sdf/stroke" | "sdf/layer" => Some("float4"),
                _ => None,
            };
        }
    }
    None
}

fn expr_returns_float4(expr: &Expression) -> bool {
    match expr {
        Expression::Keyword(_) => true,
        Expression::List(items) if items.is_empty() => false,
        Expression::List(items) => {
            let Some(Expression::Symbol(head)) = items.first() else {
                return false;
            };
            match head.as_str() {
                "vec4" | "rgba" | "sdf/fill" | "sdf/paint" | "sdf/stroke" | "sdf/layer" => true,
                "let" => items.last().is_some_and(expr_returns_float4),
                "do" => items.last().is_some_and(expr_returns_float4),
                "if" => items.get(2).is_some_and(expr_returns_float4),
                "mix" => items.get(1).is_some_and(expr_returns_float4),
                _ => false,
            }
        }
        _ => false,
    }
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

fn metal_safe_symbol(name: &str) -> String {
    name.replace('-', "_")
}

fn uniform_layout(state_symbols: &[String]) -> HashMap<String, String> {
    state_symbols
        .iter()
        .map(|name| {
            (
                name.clone(),
                format!("sdf_state_{}", metal_safe_symbol(name)),
            )
        })
        .collect()
}

fn emit_uniform_declarations(shader: &mut String, state_symbols: &[String]) {
    for (idx, name) in state_symbols.iter().enumerate() {
        let register = if idx < 4 { "uniform_a" } else { "uniform_b" };
        let component = match idx % 4 {
            0 => "x",
            1 => "y",
            2 => "z",
            _ => "w",
        };
        writeln!(
            shader,
            "    float sdf_state_{} = in.{}.{};",
            metal_safe_symbol(name),
            register,
            component
        )
        .unwrap();
    }
}

fn collect_state_symbols_impl(
    expr: &Expression,
    state_bindings: &HashSet<String>,
    scope_stack: &mut Vec<HashSet<String>>,
    out: &mut Vec<String>,
) {
    match expr {
        Expression::Symbol(name) => {
            let shadowed = scope_stack.iter().rev().any(|scope| scope.contains(name));
            if state_bindings.contains(name)
                && !shadowed
                && !out.iter().any(|existing| existing == name)
            {
                out.push(name.clone());
            }
        }
        Expression::List(items) if !items.is_empty() => {
            if let Some(Expression::Symbol(head)) = items.first()
                && head == "let"
                && items.len() >= 3
                && let Expression::List(bindings) = &items[1]
            {
                let mut scope = HashSet::new();
                for binding in bindings {
                    let Expression::List(parts) = binding else {
                        continue;
                    };
                    if parts.len() != 2 {
                        continue;
                    }
                    collect_state_symbols_impl(&parts[1], state_bindings, scope_stack, out);
                    if let Expression::Symbol(name) = &parts[0] {
                        scope.insert(name.clone());
                    }
                }
                scope_stack.push(scope);
                for body in &items[2..] {
                    collect_state_symbols_impl(body, state_bindings, scope_stack, out);
                }
                scope_stack.pop();
                return;
            }

            for arg in items.iter().skip(1) {
                collect_state_symbols_impl(arg, state_bindings, scope_stack, out);
            }
        }
        _ => {}
    }
}

pub fn collect_state_symbols(expr: &Expression, state_bindings: &HashSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut scope_stack = vec![HashSet::from([
        "x".to_string(),
        "y".to_string(),
        "d".to_string(),
        "value_t".to_string(),
        "itime".to_string(),
        "hit/hover".to_string(),
        "hit/active".to_string(),
        "hit/region".to_string(),
    ])];
    collect_state_symbols_impl(expr, state_bindings, &mut scope_stack, &mut out);
    out
}

/// Compile a macro-expanded SDF expression into a complete Metal fragment shader.
///
/// Supports both single-SDF expressions (returns distance-based rendering)
/// and `sdf/layer` expressions (returns composited multi-shape rendering with hit regions).
pub fn compile_sdf_to_metal(expr: &Expression) -> Result<SdfShaderOutput, CodegenError> {
    compile_sdf_to_metal_with_state(expr, &[])
}

pub fn compile_sdf_to_metal_with_state(
    expr: &Expression,
    state_symbols: &[String],
) -> Result<SdfShaderOutput, CodegenError> {
    let returns_color = expr_returns_float4(expr);

    let mut emitter = MetalEmitter::new(uniform_layout(state_symbols));
    let result_expr = emitter.emit_expr(expr)?;
    let region_count = emitter.next_region_id;

    let mut shader = String::with_capacity(2048);
    writeln!(
        shader,
        "fragment float4 widget_frag(WidgetVaryings in [[stage_in]])"
    )
    .unwrap();
    writeln!(shader, "{{").unwrap();
    writeln!(shader, "    float aspect = in.aspect;").unwrap();
    writeln!(
        shader,
        "    float2 logical_uv = (in.uv - in.color_c.xy) / max(in.color_c.zw - in.color_c.xy, float2(0.0001));"
    )
    .unwrap();
    writeln!(
        shader,
        "    float x = (logical_uv.x * 2.0 - 1.0) * max(aspect, 1.0);"
    )
    .unwrap();
    writeln!(
        shader,
        "    float y = (logical_uv.y * 2.0 - 1.0) * max(1.0 / max(aspect, 0.0001), 1.0);"
    )
    .unwrap();
    writeln!(shader, "    float value_t = in.value_t;").unwrap();
    writeln!(shader, "    float itime = in.itime;").unwrap();

    if region_count > 0 {
        // Hit region uniforms packed into color_b
        writeln!(shader, "    int hit_region = int(in.color_b.x);").unwrap();
        writeln!(shader, "    int hit_pressed = int(in.color_b.y);").unwrap();
    }
    emit_uniform_declarations(&mut shader, state_symbols);

    for stmt in &emitter.statements {
        writeln!(shader, "    {}", stmt).unwrap();
    }

    if returns_color {
        writeln!(shader, "    float4 result = {};", result_expr).unwrap();
        writeln!(shader, "    if (result.a < 0.001) discard_fragment();").unwrap();
        writeln!(shader, "    return result;").unwrap();
    } else {
        writeln!(shader, "    float d = {};", result_expr).unwrap();
        writeln!(shader, "    float aa = max(fwidth(d), 0.001);").unwrap();
        writeln!(shader, "    float mask = smoothstep(aa, -aa, d);").unwrap();
        writeln!(shader, "    if (mask < 0.001) discard_fragment();").unwrap();
        writeln!(shader, "    float4 fill_color = in.color_a;").unwrap();
        writeln!(
            shader,
            "    return float4(fill_color.rgb, fill_color.a * mask);"
        )
        .unwrap();
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
    let mut emitter = MetalEmitter::new(HashMap::new());
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
    fn mix_uses_arithmetic_form() {
        let (_, result) = codegen_expr("(mix :accent :white (smoothstep -1 1 y))");
        assert!(!result.contains("mix("));
        assert!(result.contains("smoothstep(-1.0, 1.0, y)"));
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
    fn rgba_constructor() {
        let (_, result) = codegen_expr("(rgba 1 0.5 0.25 0.75)");
        assert_eq!(result, "float4(1.0, 0.5, 0.25, 0.75)");
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
            vec![],
            vec![],
            vec![],
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
    fn sdf_fill_material_color() {
        let (stmts, result) =
            codegen_expr("(sdf/fill (- (length (vec2 x y)) 0.5) (material :color :accent))");
        assert!(stmts.iter().any(|s| s.contains("smoothstep")));
        assert!(stmts.iter().any(|s| s.contains("float4")));
        assert!(result.starts_with("_v"));
    }

    #[test]
    fn sdf_fill_legacy_color_is_material_sugar() {
        let legacy = compile_sdf_expr(&parse_one_expr(
            "(sdf/fill (- (length (vec2 x y)) 0.5) :accent)",
        ))
        .unwrap();
        let material = compile_sdf_expr(&parse_one_expr(
            "(sdf/fill (- (length (vec2 x y)) 0.5) (material :color :accent))",
        ))
        .unwrap();
        assert_eq!(legacy.0, material.0);
        assert_eq!(legacy.1, material.1);
    }

    #[test]
    fn material_requires_color() {
        let err = compile_sdf_expr(&parse_one_expr(
            "(sdf/fill (- (length (vec2 x y)) 0.5) (material :alpha 0.5))",
        ))
        .unwrap_err();
        assert!(format!("{}", err).contains("material requires :color"));
    }

    #[test]
    fn shadow_requires_blur() {
        let err = compile_sdf_expr(&parse_one_expr(
            "(sdf/fill (- (length (vec2 x y)) 0.5)
               (material :color :accent
                         :shadow (shadow :color (rgba 0 0 0 0.2))))",
        ))
        .unwrap_err();
        assert!(format!("{}", err).contains("shadow requires :blur"));
    }

    #[test]
    fn sdf_fill_material_color_supports_rgba() {
        let (stmts, _result) = codegen_expr(
            "(sdf/fill (- (length (vec2 x y)) 0.5) (material :color (rgba 1 0.5 0.25 0.75)))",
        );
        let all = stmts.join("\n");
        assert!(all.contains("float4(1.0, 0.5, 0.25, 0.75)"));
    }

    #[test]
    fn sdf_fill_material_color_can_use_d() {
        let (stmts, _result) = codegen_expr(
            "(sdf/fill (- (length (vec2 x y)) 0.5)
               (material :color (rgba 1 1 1 (smoothstep 0.1 -0.1 d))))",
        );
        let all = stmts.join("\n");
        assert!(all.contains("smoothstep(0.1"));
        assert!(!all.contains(", d)"));
    }

    #[test]
    fn sdf_fill_material_color_can_use_x_y_and_d() {
        let (stmts, _result) = codegen_expr(
            "(sdf/fill (- (length (vec2 x y)) 0.5)
               (material :color (rgba (smoothstep -1 1 x) (smoothstep -1 1 y) 1 (smoothstep 0.1 -0.1 d))))",
        );
        let all = stmts.join("\n");
        assert!(all.contains("smoothstep(-1.0, 1.0, x)"));
        assert!(all.contains("smoothstep(-1.0, 1.0, y)"));
        assert!(all.contains("smoothstep(0.1"));
        assert!(!all.contains(", d)"));
    }

    #[test]
    fn sdf_fill_material_shadow_compiles() {
        let (stmts, result) = codegen_expr(
            "(sdf/fill (- (length (vec2 x y)) 0.5)
               (material
                 :color :accent
                 :shadow (shadow :color (rgba 0 0 0 0.2)
                                 :blur 0.18
                                 :offset (vec2 0 0.05)
                                 :spread 0.02)))",
        );
        let all = stmts.join("\n");
        assert!(all.contains("float2"));
        assert!(all.contains("x -"));
        assert!(all.contains("y -"));
        assert!(all.contains("fwidth"));
        assert!(all.contains("smoothstep"));
        assert!(all.contains("1.0 -"));
        assert!(result.starts_with("_v"));
    }

    #[test]
    fn sdf_fill_shadow_does_not_create_extra_regions() {
        let output = compile_sdf_to_metal(&parse_one_expr(
            "(sdf/layer
               (sdf/fill (- (length (vec2 x y)) 0.5)
                 (material
                   :color :accent
                   :shadow (shadow :color (rgba 0 0 0 0.2)
                                   :blur 0.18
                                   :offset (vec2 0 0.05)))))",
        ))
        .unwrap();
        assert_eq!(output.region_count, 1);
    }

    #[test]
    fn sdf_paint_basic() {
        let (stmts, result) = codegen_expr("(sdf/paint (- (length (vec2 x y)) 0.3) :primary)");
        assert!(stmts.iter().any(|s| s.contains("smoothstep")));
        assert!(result.starts_with("_v"));
    }

    #[test]
    fn sdf_stroke_basic() {
        let (stmts, _result) =
            codegen_expr("(sdf/stroke (- (length (vec2 x y)) 0.5) 0.02 :accent)");
        // Stroke converts distance: abs(d) - width
        assert!(stmts.iter().any(|s| s.contains("abs(")));
        assert!(stmts.iter().any(|s| s.contains("0.02")));
    }

    #[test]
    fn sdf_fill_assigns_region() {
        let mut emitter = MetalEmitter::new(HashMap::new());
        let expr = parse_one_expr("(sdf/fill (- (length (vec2 x y)) 0.5) :accent)");
        let _ = emitter.emit_expr(&expr).unwrap();
        assert_eq!(emitter.next_region_id, 1);
    }

    #[test]
    fn sdf_paint_no_region() {
        let mut emitter = MetalEmitter::new(HashMap::new());
        let expr = parse_one_expr("(sdf/paint (- (length (vec2 x y)) 0.5) :accent)");
        let _ = emitter.emit_expr(&expr).unwrap();
        assert_eq!(emitter.next_region_id, 0);
    }

    #[test]
    fn sdf_fill_hit_hover() {
        let (stmts, _) =
            codegen_expr("(sdf/fill (- (length (vec2 x y)) 0.5) (if hit/hover :accent :primary))");
        // hit/hover should resolve to (hit_region == 0)
        let all = stmts.join("\n");
        assert!(all.contains("hit_region == 0"));
    }

    #[test]
    fn sdf_layer_single_fill() {
        let (stmts, result) =
            codegen_expr("(sdf/layer (sdf/fill (- (length (vec2 x y)) 0.5) :accent))");
        // Should have layer accumulator and alpha blend
        let all = stmts.join("\n");
        assert!(all.contains("float4"));
        assert!(all.contains("1.0 -"));
        assert!(result.starts_with("_v"));
    }

    #[test]
    fn sdf_layer_two_fills_get_different_regions() {
        let mut emitter = MetalEmitter::new(HashMap::new());
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

    #[test]
    fn top_level_let_returning_layer_uses_color_shader_path() {
        let output = compile_sdf_to_metal(&parse_one_expr(
            "(let ((shape (- (length (vec2 x y)) 0.5)))
               (sdf/layer
                 (sdf/fill shape (material :color :accent))))",
        ))
        .unwrap();
        let shader = &output.shader_source;
        assert!(shader.contains("float4 result ="));
        assert!(!shader.contains("float d = _v"));
        assert!(shader.contains("int hit_region"));
        assert!(shader.contains("int hit_pressed"));
        assert_eq!(output.region_count, 1);
    }

    #[test]
    fn captured_state_symbols_become_uniforms() {
        let expr = expand_sdf("(sdf/layer (sdf/fill (sdf/circle val) :accent))");
        let output = compile_sdf_to_metal_with_state(&expr, &[String::from("val")]).unwrap();
        assert!(
            output
                .shader_source
                .contains("float sdf_state_val = in.uniform_a.x;")
        );
        assert!(
            output
                .shader_source
                .contains("length(float2(x, y)) - sdf_state_val")
        );
    }

    #[test]
    fn collect_state_symbols_respects_shadowing() {
        let expr = parse_one_expr(
            "(let ((val 0.25)
                   (other amount))
               (sdf/layer
                 (sdf/fill (sdf/circle val) :accent)
                 (sdf/paint (sdf/circle amount) :primary)
                 (sdf/paint (sdf/circle other) :primary)))",
        );
        let states = HashSet::from([String::from("val"), String::from("amount")]);
        assert_eq!(
            collect_state_symbols(&expr, &states),
            vec![String::from("amount")]
        );
    }

    #[test]
    fn itime_is_available_as_builtin_uniform() {
        let expr =
            expand_sdf("(sdf/layer (sdf/fill (sdf/circle (+ 0.4 (* 0.1 (sin itime)))) :accent))");
        let output = compile_sdf_to_metal(&expr).unwrap();
        assert!(output.shader_source.contains("float itime = in.itime;"));
        assert!(output.shader_source.contains("sin(itime)"));
    }
}
