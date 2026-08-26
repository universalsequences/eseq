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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShaderLanguage {
    Metal,
    Wgsl,
}

/// Cost/quality policy for generated material lighting.
///
/// `Flat` keeps the authored color expression but supplies neutral lighting
/// bindings, avoiding all offset field samples and lighting ALU. It is intended
/// as a deliberate low-end GPU fallback, not as a different material syntax.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub enum SdfLightingQuality {
    #[default]
    Full,
    Flat,
}

impl SdfLightingQuality {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "full" => Ok(Self::Full),
            "flat" => Ok(Self::Flat),
            _ => Err(format!(
                "invalid ESEQ_SDF_LIGHTING_QUALITY value {value:?}; expected 'full' or 'flat'"
            )),
        }
    }
}

/// Options shared by the Metal and WGSL SDF emitters.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub struct SdfShaderOptions {
    pub lighting_quality: SdfLightingQuality,
}

impl SdfShaderOptions {
    pub const fn flat_lighting() -> Self {
        Self {
            lighting_quality: SdfLightingQuality::Flat,
        }
    }

    /// Read the process-level quality override used by application runtimes.
    /// An absent variable preserves full authored lighting.
    pub fn from_env() -> Result<Self, String> {
        let lighting_quality = match std::env::var("ESEQ_SDF_LIGHTING_QUALITY") {
            Ok(value) => SdfLightingQuality::parse(&value)?,
            Err(std::env::VarError::NotPresent) => SdfLightingQuality::Full,
            Err(error) => return Err(format!("cannot read ESEQ_SDF_LIGHTING_QUALITY: {error}")),
        };
        Ok(Self { lighting_quality })
    }
}

struct ShaderEmitter {
    language: ShaderLanguage,
    options: SdfShaderOptions,
    next_var: usize,
    statements: Vec<String>,
    scopes: Vec<HashMap<String, String>>,
    type_scopes: Vec<HashMap<String, &'static str>>,
    uniform_symbols: HashMap<String, String>,
    theme: theme::Theme,
    current_region_id: Option<usize>,
    next_region_id: usize,
}

struct MaterialSpec<'a> {
    color_expr: &'a Expression,
    shadow: Option<ShadowSpec<'a>>,
    lighting: Option<LightingSpec<'a>>,
}

struct ShadowSpec<'a> {
    color_expr: &'a Expression,
    blur_expr: &'a Expression,
    offset_expr: Option<&'a Expression>,
    spread_expr: Option<&'a Expression>,
}

struct LightingSpec<'a> {
    edge_min_expr: &'a Expression,
    edge_max_expr: &'a Expression,
    eps_expr: Option<&'a Expression>,
    light_expr: Option<&'a Expression>,
    shininess_expr: Option<&'a Expression>,
    bump_expr: Option<&'a Expression>,
}

impl ShaderEmitter {
    fn metal(uniform_symbols: HashMap<String, String>) -> Self {
        Self::metal_with_theme(uniform_symbols, theme::current())
    }

    fn metal_with_theme(uniform_symbols: HashMap<String, String>, theme: theme::Theme) -> Self {
        Self::metal_with_theme_and_options(uniform_symbols, theme, SdfShaderOptions::default())
    }

    fn metal_with_theme_and_options(
        uniform_symbols: HashMap<String, String>,
        theme: theme::Theme,
        options: SdfShaderOptions,
    ) -> Self {
        Self::new(ShaderLanguage::Metal, uniform_symbols, theme, options)
    }

    fn wgsl(uniform_symbols: HashMap<String, String>) -> Self {
        Self::wgsl_with_theme(uniform_symbols, theme::current())
    }

    fn wgsl_with_theme(uniform_symbols: HashMap<String, String>, theme: theme::Theme) -> Self {
        Self::wgsl_with_theme_and_options(uniform_symbols, theme, SdfShaderOptions::default())
    }

    fn wgsl_with_theme_and_options(
        uniform_symbols: HashMap<String, String>,
        theme: theme::Theme,
        options: SdfShaderOptions,
    ) -> Self {
        Self::new(ShaderLanguage::Wgsl, uniform_symbols, theme, options)
    }

    fn new(
        language: ShaderLanguage,
        uniform_symbols: HashMap<String, String>,
        theme: theme::Theme,
        options: SdfShaderOptions,
    ) -> Self {
        Self {
            language,
            options,
            next_var: 0,
            statements: Vec::new(),
            scopes: Vec::new(),
            type_scopes: Vec::new(),
            uniform_symbols,
            theme,
            current_region_id: None,
            next_region_id: 0,
        }
    }

    fn fresh_var(&mut self) -> String {
        let name = format!("_v{}", self.next_var);
        self.next_var += 1;
        name
    }

    fn input_name(&self) -> &'static str {
        match self.language {
            ShaderLanguage::Metal => "in",
            ShaderLanguage::Wgsl => "input",
        }
    }

    fn type_name(&self, metal_name: &'static str) -> &'static str {
        match (self.language, metal_name) {
            (ShaderLanguage::Wgsl, "float") => "f32",
            (ShaderLanguage::Wgsl, "float2") => "vec2<f32>",
            (ShaderLanguage::Wgsl, "float3") => "vec3<f32>",
            (ShaderLanguage::Wgsl, "float4") => "vec4<f32>",
            _ => metal_name,
        }
    }

    fn declaration(&self, type_name: &'static str, name: &str, value: &str) -> String {
        match self.language {
            ShaderLanguage::Metal => format!("{} {} = {};", type_name, name, value),
            ShaderLanguage::Wgsl => {
                format!("let {}: {} = {};", name, self.type_name(type_name), value)
            }
        }
    }

    fn mutable_declaration(&self, type_name: &'static str, name: &str, value: &str) -> String {
        match self.language {
            ShaderLanguage::Metal => format!("{} {} = {};", type_name, name, value),
            ShaderLanguage::Wgsl => {
                format!("var {}: {} = {};", name, self.type_name(type_name), value)
            }
        }
    }

    fn constructor(&self, type_name: &'static str) -> &'static str {
        self.type_name(type_name)
    }

    fn resolve_symbol(&self, name: &str) -> String {
        // Hit contextual variables
        match name {
            "itime" => return "itime".to_string(),
            "aspect" => return "aspect".to_string(),
            "input-color" => return format!("{}.color_a", self.input_name()),
            "hit/hover" => {
                return if let Some(rid) = self.current_region_id {
                    match self.language {
                        ShaderLanguage::Metal => format!("(hit_region == {})", rid),
                        ShaderLanguage::Wgsl => format!("(hit_region == {rid}.0)"),
                    }
                } else {
                    "false".to_string()
                };
            }
            "hit/active" => {
                return if let Some(rid) = self.current_region_id {
                    match self.language {
                        ShaderLanguage::Metal => {
                            format!("((hit_region == {}) && (hit_pressed != 0))", rid)
                        }
                        ShaderLanguage::Wgsl => {
                            format!("((hit_region == {rid}.0) && hit_pressed)")
                        }
                    }
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

    fn resolve_symbol_type(&self, name: &str) -> Option<&'static str> {
        for scope in self.type_scopes.iter().rev() {
            if let Some(type_name) = scope.get(name) {
                return Some(*type_name);
            }
        }
        None
    }

    fn expr_is_bool(&self, expr: &Expression) -> bool {
        match expr {
            Expression::Symbol(name) => {
                matches!(name.as_str(), "true" | "false" | "hit/hover" | "hit/active")
                    || self.resolve_symbol_type(name) == Some("bool")
            }
            Expression::List(items) => matches!(
                items.first(),
                Some(Expression::Symbol(head))
                    if matches!(head.as_str(), "=" | "<" | ">" | "<=" | ">=")
            ),
            _ => false,
        }
    }

    fn expr_type(&self, expr: &Expression) -> Option<&'static str> {
        match expr {
            Expression::Keyword(_) => Some("float4"),
            Expression::Symbol(name) if name == "input-color" => Some("float4"),
            Expression::Symbol(name) => self.resolve_symbol_type(name),
            Expression::List(items) if items.is_empty() => None,
            Expression::List(items) => {
                let Some(Expression::Symbol(head)) = items.first() else {
                    return None;
                };
                match head.as_str() {
                    "vec2" => Some("float2"),
                    "vec3" => Some("float3"),
                    "vec4" | "rgba" | "sdf/fill" | "sdf/paint" | "sdf/stroke" | "sdf/layer" => {
                        Some("float4")
                    }
                    "let" | "do" => items.last().and_then(|expr| self.expr_type(expr)),
                    "if" => items
                        .get(2)
                        .and_then(|expr| self.expr_type(expr))
                        .or_else(|| items.get(3).and_then(|expr| self.expr_type(expr))),
                    "mix" => items
                        .get(1)
                        .and_then(|expr| self.expr_type(expr))
                        .or_else(|| items.get(2).and_then(|expr| self.expr_type(expr))),
                    // normalize preserves its input type (float2→float2, float3→float3)
                    "normalize" => items.get(1).and_then(|expr| self.expr_type(expr)),
                    // + and * propagate type from first operand
                    "+" | "-" | "*" | "/" => items.get(1).and_then(|expr| self.expr_type(expr)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn emit_expr(&mut self, expr: &Expression) -> Result<String, CodegenError> {
        match expr {
            Expression::Number(n) => Ok(format_float(*n)),

            Expression::Symbol(name) => Ok(self.resolve_symbol(name)),

            Expression::Keyword(name) => {
                let color = theme::named_color_in(&self.theme, name)
                    .ok_or_else(|| CodegenError::UnknownThemeColor(name.clone()))?;
                Ok(format!(
                    "{}({}, {}, {}, {})",
                    self.constructor("float4"),
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
                    "vec2" => self.emit_func_call(self.constructor("float2"), args),
                    "vec3" => self.emit_func_call(self.constructor("float3"), args),
                    "vec4" => self.emit_func_call(self.constructor("float4"), args),
                    "rgba" => self.emit_func_call(self.constructor("float4"), args),

                    // 1-arg math intrinsics (same name in Metal)
                    "abs" | "sin" | "cos" | "sqrt" | "fract" | "floor" | "ceil" | "round"
                    | "length" | "normalize" | "fwidth" => self.emit_func_call(head, args),

                    // 2-arg math intrinsics
                    "pow" | "atan2" | "dot" => self.emit_func_call(head, args),
                    "mod" if self.language == ShaderLanguage::Metal => {
                        self.emit_func_call("fmod", args)
                    }
                    "mod" => self.emit_binary_op("%", args),

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
            Expression::Quasiquote(_) | Expression::Unquote(_) | Expression::UnquoteSplicing(_) => {
                Err(CodegenError::UnsupportedExpression(
                    "unexpanded quasiquote/unquote".into(),
                ))
            }

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
        let zero = format!("{}(0.0, 0.0, 0.0, 0.0)", self.constructor("float4"));
        self.statements
            .push(self.mutable_declaration("float4", &layer_var, &zero));

        for child in children {
            let child_color = self.emit_expr(child)?;
            let c = self.fresh_var();
            self.statements
                .push(self.declaration("float4", &c, &child_color));
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
        self.statements.push(self.declaration("float", &d, &dist));

        // AA mask
        let aa = self.fresh_var();
        let aa_expr = format!("max(fwidth({}), 0.001)", d);
        self.statements
            .push(self.declaration("float", &aa, &aa_expr));
        let mask = self.fresh_var();
        let mask_expr = format!("smoothstep({}, -({}), {})", aa, aa, d);
        self.statements
            .push(self.declaration("float", &mask, &mask_expr));

        let material = self.parse_material(material_expr)?;
        let mut scope_bindings = HashMap::from([("d".to_string(), d.clone())]);

        // Emit lighting contribution (normal estimation + optional diffuse/specular)
        // before pushing the scope, so lighting bindings are available in color expr
        let lighting_bindings = if let Some(ref lighting) = material.lighting {
            match self.options.lighting_quality {
                SdfLightingQuality::Full => self.emit_lighting_contribution(sdf_expr, lighting)?,
                SdfLightingQuality::Flat => self.emit_flat_lighting_contribution(lighting),
            }
        } else {
            HashMap::new()
        };
        scope_bindings.extend(lighting_bindings);

        self.scopes.push(scope_bindings);
        self.type_scopes.push(HashMap::new());
        // Register normal as float3 in type scope if present
        if material.lighting.is_some() {
            self.type_scopes
                .last_mut()
                .unwrap()
                .insert("normal".to_string(), "float3");
        }
        let shadow = self.emit_shadow_contribution(sdf_expr, &material, &d)?;
        let color = self.emit_expr(material.color_expr)?;
        self.type_scopes.pop();
        self.scopes.pop();
        let clr = self.fresh_var();
        self.statements
            .push(self.declaration("float4", &clr, &color));

        let fill_result = self.fresh_var();
        let fill_expr = format!(
            "{}({}.rgb * {}.a * {}, {}.a * {})",
            self.constructor("float4"),
            clr,
            clr,
            mask,
            clr,
            mask
        );
        self.statements
            .push(self.declaration("float4", &fill_result, &fill_expr));

        let result = self.fresh_var();
        if let Some(shadow) = shadow {
            let result_expr = format!("{} + {} * (1.0 - {}.a)", fill_result, shadow, fill_result);
            self.statements
                .push(self.declaration("float4", &result, &result_expr));
        } else {
            self.statements
                .push(self.declaration("float4", &result, &fill_result));
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
                lighting: None,
            });
        };
        let Some(Expression::Symbol(head)) = items.first() else {
            return Ok(MaterialSpec {
                color_expr: expr,
                shadow: None,
                lighting: None,
            });
        };
        if head != "material" {
            return Ok(MaterialSpec {
                color_expr: expr,
                shadow: None,
                lighting: None,
            });
        }

        let mut color_expr = None;
        let mut shadow_expr = None;
        let mut lighting_expr = None;
        let mut i = 1;
        while i + 1 < items.len() {
            if let Expression::Keyword(key) = &items[i] {
                match key.as_str() {
                    "color" => color_expr = Some(&items[i + 1]),
                    "shadow" => shadow_expr = Some(&items[i + 1]),
                    "lighting" => lighting_expr = Some(&items[i + 1]),
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
            lighting: lighting_expr.map(Self::parse_lighting).transpose()?,
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

    fn parse_lighting<'a>(expr: &'a Expression) -> Result<LightingSpec<'a>, CodegenError> {
        let Expression::List(items) = expr else {
            return Err(CodegenError::UnsupportedExpression(
                "lighting must be a form".into(),
            ));
        };
        let Some(Expression::Symbol(head)) = items.first() else {
            return Err(CodegenError::UnsupportedExpression(
                "lighting head must be a symbol".into(),
            ));
        };
        if head != "lighting" {
            return Err(CodegenError::UnsupportedExpression(
                "material :lighting must use lighting".into(),
            ));
        }

        let mut edge_min_expr = None;
        let mut edge_max_expr = None;
        let mut eps_expr = None;
        let mut light_expr = None;
        let mut shininess_expr = None;
        let mut bump_expr = None;
        let mut i = 1;
        while i + 1 < items.len() {
            if let Expression::Keyword(key) = &items[i] {
                match key.as_str() {
                    "edge-min" => edge_min_expr = Some(&items[i + 1]),
                    "edge-max" => edge_max_expr = Some(&items[i + 1]),
                    "eps" => eps_expr = Some(&items[i + 1]),
                    "light" => light_expr = Some(&items[i + 1]),
                    "shininess" => shininess_expr = Some(&items[i + 1]),
                    "bump" => bump_expr = Some(&items[i + 1]),
                    _ => {}
                }
                i += 2;
            } else {
                i += 1;
            }
        }

        let Some(edge_min_expr) = edge_min_expr else {
            return Err(CodegenError::UnsupportedExpression(
                "lighting requires :edge-min".into(),
            ));
        };
        let Some(edge_max_expr) = edge_max_expr else {
            return Err(CodegenError::UnsupportedExpression(
                "lighting requires :edge-max".into(),
            ));
        };

        Ok(LightingSpec {
            edge_min_expr,
            edge_max_expr,
            eps_expr,
            light_expr,
            shininess_expr,
            bump_expr,
        })
    }

    /// Supply neutral bindings for an authored lighting block without evaluating
    /// its field taps or light model. Keeping these names available means flat
    /// quality works for color expressions that use `normal`, `diffuse`, or
    /// `specular`, rather than requiring content-specific alternate materials.
    fn emit_flat_lighting_contribution(
        &mut self,
        lighting: &LightingSpec<'_>,
    ) -> HashMap<String, String> {
        let normal = self.fresh_var();
        let normal_expr = format!("{}(0.0, 0.0, 1.0)", self.constructor("float3"));
        self.statements
            .push(self.declaration("float3", &normal, &normal_expr));

        let mut bindings = HashMap::from([("normal".to_string(), normal)]);
        if lighting.light_expr.is_some() {
            let diffuse = self.fresh_var();
            self.statements
                .push(self.declaration("float", &diffuse, "1.0"));
            bindings.insert("diffuse".to_string(), diffuse);

            let specular = self.fresh_var();
            self.statements
                .push(self.declaration("float", &specular, "0.0"));
            bindings.insert("specular".to_string(), specular);
        }
        bindings
    }

    /// Emit normal estimation and optional diffuse/specular.
    /// Returns a HashMap of variable bindings to inject into the color scope.
    fn emit_lighting_contribution(
        &mut self,
        sdf_expr: &Expression,
        lighting: &LightingSpec<'_>,
    ) -> Result<HashMap<String, String>, CodegenError> {
        let mut bindings = HashMap::new();

        let edge_min = self.emit_expr(lighting.edge_min_expr)?;
        let edge_max = self.emit_expr(lighting.edge_max_expr)?;
        let eps = match lighting.eps_expr {
            Some(expr) => self.emit_expr(expr)?,
            None => "0.01".to_string(),
        };

        // Sample SDF at 4 offset positions, apply smoothstep to create height field.
        // If :bump is provided, evaluate it at each offset and add to the height.
        // This creates surface detail without changing the shape boundary.

        // Right: x + eps
        let right_x = self.fresh_var();
        let right_x_expr = format!("x + {}", eps);
        self.statements
            .push(self.declaration("float", &right_x, &right_x_expr));
        self.scopes
            .push(HashMap::from([("x".to_string(), right_x.clone())]));
        let right_sdf = self.emit_expr(sdf_expr)?;
        let right_bump = if let Some(bump) = lighting.bump_expr {
            Some(self.emit_expr(bump)?)
        } else {
            None
        };
        self.scopes.pop();
        let right = self.fresh_var();
        let right_expr = if let Some(ref rb) = right_bump {
            format!(
                "smoothstep({}, {}, {}) + {}",
                edge_min, edge_max, right_sdf, rb
            )
        } else {
            format!("smoothstep({}, {}, {})", edge_min, edge_max, right_sdf)
        };
        self.statements
            .push(self.declaration("float", &right, &right_expr));

        // Left: x - eps
        let left_x = self.fresh_var();
        let left_x_expr = format!("x - {}", eps);
        self.statements
            .push(self.declaration("float", &left_x, &left_x_expr));
        self.scopes
            .push(HashMap::from([("x".to_string(), left_x.clone())]));
        let left_sdf = self.emit_expr(sdf_expr)?;
        let left_bump = if let Some(bump) = lighting.bump_expr {
            Some(self.emit_expr(bump)?)
        } else {
            None
        };
        self.scopes.pop();
        let left = self.fresh_var();
        let left_expr = if let Some(ref lb) = left_bump {
            format!(
                "smoothstep({}, {}, {}) + {}",
                edge_min, edge_max, left_sdf, lb
            )
        } else {
            format!("smoothstep({}, {}, {})", edge_min, edge_max, left_sdf)
        };
        self.statements
            .push(self.declaration("float", &left, &left_expr));

        // Up: y + eps
        let up_y = self.fresh_var();
        let up_y_expr = format!("y + {}", eps);
        self.statements
            .push(self.declaration("float", &up_y, &up_y_expr));
        self.scopes
            .push(HashMap::from([("y".to_string(), up_y.clone())]));
        let up_sdf = self.emit_expr(sdf_expr)?;
        let up_bump = if let Some(bump) = lighting.bump_expr {
            Some(self.emit_expr(bump)?)
        } else {
            None
        };
        self.scopes.pop();
        let up = self.fresh_var();
        let up_expr = if let Some(ref ub) = up_bump {
            format!(
                "smoothstep({}, {}, {}) + {}",
                edge_min, edge_max, up_sdf, ub
            )
        } else {
            format!("smoothstep({}, {}, {})", edge_min, edge_max, up_sdf)
        };
        self.statements
            .push(self.declaration("float", &up, &up_expr));

        // Down: y - eps
        let down_y = self.fresh_var();
        let down_y_expr = format!("y - {}", eps);
        self.statements
            .push(self.declaration("float", &down_y, &down_y_expr));
        self.scopes
            .push(HashMap::from([("y".to_string(), down_y.clone())]));
        let down_sdf = self.emit_expr(sdf_expr)?;
        let down_bump = if let Some(bump) = lighting.bump_expr {
            Some(self.emit_expr(bump)?)
        } else {
            None
        };
        self.scopes.pop();
        let down = self.fresh_var();
        let down_expr = if let Some(ref db) = down_bump {
            format!(
                "smoothstep({}, {}, {}) + {}",
                edge_min, edge_max, down_sdf, db
            )
        } else {
            format!("smoothstep({}, {}, {})", edge_min, edge_max, down_sdf)
        };
        self.statements
            .push(self.declaration("float", &down, &down_expr));

        // Central differences → normal
        let dx = self.fresh_var();
        let dx_expr = format!("({} - {}) / (2.0 * {})", right, left, eps);
        self.statements
            .push(self.declaration("float", &dx, &dx_expr));
        let dy = self.fresh_var();
        let dy_expr = format!("({} - {}) / (2.0 * {})", up, down, eps);
        self.statements
            .push(self.declaration("float", &dy, &dy_expr));
        let normal = self.fresh_var();
        let normal_expr = format!(
            "normalize({}({}, {}, 1.0))",
            self.constructor("float3"),
            dx,
            dy
        );
        self.statements
            .push(self.declaration("float3", &normal, &normal_expr));
        self.type_scopes
            .last_mut()
            .map(|s| s.insert("normal".to_string(), "float3"));
        bindings.insert("normal".to_string(), normal.clone());

        // If :light is provided, also compute diffuse and specular
        if let Some(light_expr) = lighting.light_expr {
            let light = self.emit_expr(light_expr)?;
            let light_var = self.fresh_var();
            let light_expr = format!("normalize({})", light);
            self.statements
                .push(self.declaration("float3", &light_var, &light_expr));

            // Diffuse: max(0, dot(normal, light))
            let diffuse = self.fresh_var();
            let diffuse_expr = format!("max(0.0, dot({}, {}))", normal, light_var);
            self.statements
                .push(self.declaration("float", &diffuse, &diffuse_expr));
            bindings.insert("diffuse".to_string(), diffuse);

            // Specular: Blinn-Phong
            let shininess = match lighting.shininess_expr {
                Some(expr) => self.emit_expr(expr)?,
                None => "48.0".to_string(),
            };
            let half_vec = self.fresh_var();
            let half_expr = format!(
                "normalize({} + {}(0.0, 0.0, 1.0))",
                light_var,
                self.constructor("float3")
            );
            self.statements
                .push(self.declaration("float3", &half_vec, &half_expr));
            let specular = self.fresh_var();
            let specular_expr = format!(
                "pow(max(0.0, dot({}, {})), {})",
                normal, half_vec, shininess
            );
            self.statements
                .push(self.declaration("float", &specular, &specular_expr));
            bindings.insert("specular".to_string(), specular);
        }

        Ok(bindings)
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
            .push(self.declaration("float2", &offset_var, &offset));

        let shadow_x = self.fresh_var();
        let shadow_x_expr = format!("x - {}.x", offset_var);
        self.statements
            .push(self.declaration("float", &shadow_x, &shadow_x_expr));
        let shadow_y = self.fresh_var();
        let shadow_y_expr = format!("y - {}.y", offset_var);
        self.statements
            .push(self.declaration("float", &shadow_y, &shadow_y_expr));

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
        let shadow_d_expr = format!("({} - {})", shadow_dist_expr, spread_expr);
        self.statements
            .push(self.declaration("float", &shadow_d, &shadow_d_expr));
        let shadow_soft = self.fresh_var();
        let shadow_soft_expr = format!("max(max({}, fwidth({})), 0.001)", blur_expr, shadow_d);
        self.statements
            .push(self.declaration("float", &shadow_soft, &shadow_soft_expr));
        let shadow_mask = self.fresh_var();
        let shadow_mask_expr = format!(
            "smoothstep({}, -({}), {})",
            shadow_soft, shadow_soft, shadow_d
        );
        self.statements
            .push(self.declaration("float", &shadow_mask, &shadow_mask_expr));
        let shadow_color = self.fresh_var();
        self.statements
            .push(self.declaration("float4", &shadow_color, &shadow_color_expr));
        let shadow_result = self.fresh_var();
        let shadow_result_expr = format!(
            "{}({}.rgb * {}.a * {}, {}.a * {})",
            self.constructor("float4"),
            shadow_color,
            shadow_color,
            shadow_mask,
            shadow_color,
            shadow_mask
        );
        self.statements
            .push(self.declaration("float4", &shadow_result, &shadow_result_expr));
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
        self.statements.push(self.declaration("float", &d, &dist));

        // Convert to stroke: abs(d) - width
        let width = self.emit_expr(&args[1])?;
        let stroke_d = self.fresh_var();
        let stroke_expr = format!("abs({}) - {}", d, width);
        self.statements
            .push(self.declaration("float", &stroke_d, &stroke_expr));

        // AA mask
        let aa = self.fresh_var();
        let aa_expr = format!("max(fwidth({}), 0.001)", stroke_d);
        self.statements
            .push(self.declaration("float", &aa, &aa_expr));
        let mask = self.fresh_var();
        let mask_expr = format!("smoothstep({}, -({}), {})", aa, aa, stroke_d);
        self.statements
            .push(self.declaration("float", &mask, &mask_expr));

        // Evaluate color
        let color = self.emit_expr(&args[2])?;
        let clr = self.fresh_var();
        self.statements
            .push(self.declaration("float4", &clr, &color));

        // Premultiplied alpha output
        let result = self.fresh_var();
        let result_expr = format!(
            "{}({}.rgb * {}.a * {}, {}.a * {})",
            self.constructor("float4"),
            clr,
            clr,
            mask,
            clr,
            mask
        );
        self.statements
            .push(self.declaration("float4", &result, &result_expr));

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
        self.type_scopes.push(HashMap::new());
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
            let type_name = if self.language == ShaderLanguage::Wgsl && self.expr_is_bool(&pair[1])
            {
                "bool"
            } else {
                self.expr_type(&pair[1]).unwrap_or("float")
            };
            self.statements
                .push(self.declaration(type_name, &var, &val));
            self.scopes.last_mut().unwrap().insert(name.clone(), var);
            if type_name != "float" {
                self.type_scopes
                    .last_mut()
                    .unwrap()
                    .insert(name.clone(), type_name);
            }
        }

        let body = self.emit_body(&args[1..])?;
        self.scopes.pop();
        self.type_scopes.pop();
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
        match self.language {
            ShaderLanguage::Metal => Ok(format!("(({}) ? ({}) : ({}))", cond, then, else_)),
            ShaderLanguage::Wgsl => {
                let condition = if self.expr_is_bool(&args[0]) {
                    cond
                } else {
                    format!("({cond} != 0.0)")
                };
                Ok(format!("select(({}), ({}), ({}))", else_, then, condition))
            }
        }
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

fn emit_metal_uniform_declarations(shader: &mut String, state_symbols: &[String]) {
    for (idx, name) in state_symbols.iter().enumerate() {
        let register = match idx / 4 {
            0 => "uniform_a",
            1 => "uniform_b",
            2 => "uniform_c",
            _ => "uniform_d",
        };
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
        "aspect".to_string(),
        "hit/hover".to_string(),
        "hit/active".to_string(),
        "hit/region".to_string(),
        "input-color".to_string(),
    ])];
    collect_state_symbols_impl(expr, state_bindings, &mut scope_stack, &mut out);
    out
}

/// Compile a macro-expanded SDF expression into a complete Metal fragment shader.
///
/// Supports both single-SDF expressions (returns distance-based rendering)
/// and `sdf/layer` expressions (returns composited multi-shape rendering with hit regions).
pub fn compile_sdf_to_metal(expr: &Expression) -> Result<SdfShaderOutput, CodegenError> {
    compile_sdf_to_metal_with_options(expr, SdfShaderOptions::default())
}

pub fn compile_sdf_to_metal_with_options(
    expr: &Expression,
    options: SdfShaderOptions,
) -> Result<SdfShaderOutput, CodegenError> {
    compile_sdf_to_metal_with_state_and_options(expr, &[], options)
}

pub fn compile_sdf_to_metal_with_state(
    expr: &Expression,
    state_symbols: &[String],
) -> Result<SdfShaderOutput, CodegenError> {
    compile_sdf_to_metal_with_state_and_options(expr, state_symbols, SdfShaderOptions::default())
}

pub fn compile_sdf_to_metal_with_state_and_options(
    expr: &Expression,
    state_symbols: &[String],
    options: SdfShaderOptions,
) -> Result<SdfShaderOutput, CodegenError> {
    compile_sdf_to_metal_with_state_theme_and_options(
        expr,
        state_symbols,
        theme::current(),
        options,
    )
}

/// Same as [`compile_sdf_to_metal_with_state`], but against an explicit theme
/// instead of the process-global one. Tests use this so a concurrently running
/// test that swaps the active theme cannot perturb their output.
fn compile_sdf_to_metal_with_state_and_theme(
    expr: &Expression,
    state_symbols: &[String],
    theme: theme::Theme,
) -> Result<SdfShaderOutput, CodegenError> {
    compile_sdf_to_metal_with_state_theme_and_options(
        expr,
        state_symbols,
        theme,
        SdfShaderOptions::default(),
    )
}

fn compile_sdf_to_metal_with_state_theme_and_options(
    expr: &Expression,
    state_symbols: &[String],
    theme: theme::Theme,
    options: SdfShaderOptions,
) -> Result<SdfShaderOutput, CodegenError> {
    let returns_color = expr_returns_float4(expr);

    let mut emitter = ShaderEmitter::metal_with_theme_and_options(
        uniform_layout(state_symbols),
        theme,
        options,
    );
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
    // Half-extents of the SDF coordinate space — use these to size shapes
    // that should fill the widget (e.g. sdf/rounded-rect (* width 0.98) (* height 0.98) r)
    writeln!(shader, "    float width = max(aspect, 1.0);").unwrap();
    writeln!(
        shader,
        "    float height = max(1.0 / max(aspect, 0.0001), 1.0);"
    )
    .unwrap();
    writeln!(shader, "    float value_t = in.value_t;").unwrap();
    writeln!(shader, "    float itime = in.itime;").unwrap();

    if region_count > 0 {
        // Hit region uniforms packed into color_b
        writeln!(shader, "    int hit_region = int(in.color_b.x);").unwrap();
        writeln!(shader, "    int hit_pressed = int(in.color_b.y);").unwrap();
    }
    emit_metal_uniform_declarations(&mut shader, state_symbols);

    for stmt in &emitter.statements {
        writeln!(shader, "    {}", stmt).unwrap();
    }

    if returns_color {
        writeln!(shader, "    float4 result = {};", result_expr).unwrap();
        writeln!(
            shader,
            "    float style_brightness = in.color_b.w <= 0.0 ? 1.0 : in.color_b.w;"
        )
        .unwrap();
        writeln!(shader, "    result.rgb *= style_brightness;").unwrap();
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
            "    float style_brightness = in.color_b.w <= 0.0 ? 1.0 : in.color_b.w;"
        )
        .unwrap();
        writeln!(shader, "    fill_color.rgb *= style_brightness;").unwrap();
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

/// Compile a macro-expanded SDF expression into a complete WGSL fragment shader.
pub fn compile_sdf_to_wgsl(expr: &Expression) -> Result<SdfShaderOutput, CodegenError> {
    compile_sdf_to_wgsl_with_options(expr, SdfShaderOptions::default())
}

pub fn compile_sdf_to_wgsl_with_options(
    expr: &Expression,
    options: SdfShaderOptions,
) -> Result<SdfShaderOutput, CodegenError> {
    compile_sdf_to_wgsl_with_state_and_options(expr, &[], options)
}

pub fn compile_sdf_to_wgsl_with_state(
    expr: &Expression,
    state_symbols: &[String],
) -> Result<SdfShaderOutput, CodegenError> {
    compile_sdf_to_wgsl_with_state_and_options(expr, state_symbols, SdfShaderOptions::default())
}

pub fn compile_sdf_to_wgsl_with_state_and_options(
    expr: &Expression,
    state_symbols: &[String],
    options: SdfShaderOptions,
) -> Result<SdfShaderOutput, CodegenError> {
    compile_sdf_to_wgsl_with_state_theme_and_options(
        expr,
        state_symbols,
        theme::current(),
        options,
    )
}

/// Theme-pinned counterpart to [`compile_sdf_to_wgsl_with_state`]. See
/// [`compile_sdf_to_metal_with_state_and_theme`].
fn compile_sdf_to_wgsl_with_state_and_theme(
    expr: &Expression,
    state_symbols: &[String],
    theme: theme::Theme,
) -> Result<SdfShaderOutput, CodegenError> {
    compile_sdf_to_wgsl_with_state_theme_and_options(
        expr,
        state_symbols,
        theme,
        SdfShaderOptions::default(),
    )
}

fn compile_sdf_to_wgsl_with_state_theme_and_options(
    expr: &Expression,
    state_symbols: &[String],
    theme: theme::Theme,
    options: SdfShaderOptions,
) -> Result<SdfShaderOutput, CodegenError> {
    let returns_color = expr_returns_float4(expr);
    let mut emitter = ShaderEmitter::wgsl_with_theme_and_options(
        uniform_layout(state_symbols),
        theme,
        options,
    );
    let result_expr = emitter.emit_expr(expr)?;
    let region_count = emitter.next_region_id;

    // `WidgetVaryings` is declared once by
    // `crate::ui::wgsl_shaders::WIDGET_SHADER_PREAMBLE_WGSL`, which every
    // widget module is assembled onto — exactly as the MSL emitter relies on
    // `WIDGET_SHADER_PREAMBLE`. Emitting a second copy here would make the
    // assembled module fail to compile on a duplicate struct.
    let mut shader = String::with_capacity(3072);
    writeln!(
        shader,
        "@fragment\nfn widget_frag(input: WidgetVaryings) -> @location(0) vec4<f32> {{"
    )
    .unwrap();
    writeln!(shader, "    let aspect: f32 = input.aspect;").unwrap();
    writeln!(
        shader,
        "    let logical_uv: vec2<f32> = (input.uv - input.color_c.xy) / max(input.color_c.zw - input.color_c.xy, vec2<f32>(0.0001));"
    )
    .unwrap();
    writeln!(
        shader,
        "    let x: f32 = (logical_uv.x * 2.0 - 1.0) * max(aspect, 1.0);"
    )
    .unwrap();
    writeln!(
        shader,
        "    let y: f32 = (logical_uv.y * 2.0 - 1.0) * max(1.0 / max(aspect, 0.0001), 1.0);"
    )
    .unwrap();
    writeln!(shader, "    let width: f32 = max(aspect, 1.0);").unwrap();
    writeln!(
        shader,
        "    let height: f32 = max(1.0 / max(aspect, 0.0001), 1.0);"
    )
    .unwrap();
    writeln!(shader, "    let value_t: f32 = input.value_t;").unwrap();
    writeln!(shader, "    let itime: f32 = input.itime;").unwrap();

    if region_count > 0 {
        writeln!(shader, "    let hit_region: f32 = input.color_b.x;").unwrap();
        writeln!(
            shader,
            "    let hit_pressed: bool = input.color_b.y != 0.0;"
        )
        .unwrap();
    }
    for (idx, name) in state_symbols.iter().enumerate() {
        let register = match idx / 4 {
            0 => "uniform_a",
            1 => "uniform_b",
            2 => "uniform_c",
            _ => "uniform_d",
        };
        let component = ["x", "y", "z", "w"][idx % 4];
        writeln!(
            shader,
            "    let sdf_state_{}: f32 = input.{}.{};",
            metal_safe_symbol(name),
            register,
            component
        )
        .unwrap();
    }

    for stmt in &emitter.statements {
        writeln!(shader, "    {}", stmt).unwrap();
    }

    if returns_color {
        writeln!(shader, "    var result: vec4<f32> = {};", result_expr).unwrap();
        writeln!(
            shader,
            "    let style_brightness: f32 = select(input.color_b.w, 1.0, input.color_b.w <= 0.0);"
        )
        .unwrap();
        writeln!(
            shader,
            "    result = vec4<f32>(result.rgb * style_brightness, result.a);"
        )
        .unwrap();
        writeln!(shader, "    if (result.a < 0.001) {{ discard; }}").unwrap();
        writeln!(shader, "    return result;").unwrap();
    } else {
        writeln!(shader, "    let d: f32 = {};", result_expr).unwrap();
        writeln!(shader, "    let aa: f32 = max(fwidth(d), 0.001);").unwrap();
        writeln!(shader, "    let mask: f32 = smoothstep(aa, -aa, d);").unwrap();
        writeln!(shader, "    if (mask < 0.001) {{ discard; }}").unwrap();
        writeln!(shader, "    let fill_color: vec4<f32> = input.color_a;").unwrap();
        writeln!(
            shader,
            "    let style_brightness: f32 = select(input.color_b.w, 1.0, input.color_b.w <= 0.0);"
        )
        .unwrap();
        writeln!(
            shader,
            "    return vec4<f32>(fill_color.rgb * style_brightness, fill_color.a * mask);"
        )
        .unwrap();
    }
    writeln!(shader, "}}").unwrap();

    Ok(SdfShaderOutput {
        shader_source: shader,
        region_count,
    })
}

/// Compile only the Metal SDF expression (no shader wrapper).
pub fn compile_sdf_expr(expr: &Expression) -> Result<(Vec<String>, String), CodegenError> {
    compile_sdf_expr_with_theme(expr, theme::current())
}

/// Compile only the WGSL SDF expression (no shader wrapper).
pub fn compile_sdf_expr_to_wgsl(expr: &Expression) -> Result<(Vec<String>, String), CodegenError> {
    let mut emitter = ShaderEmitter::wgsl(HashMap::new());
    let result = emitter.emit_expr(expr)?;
    Ok((emitter.statements, result))
}

fn compile_sdf_expr_with_theme(
    expr: &Expression,
    theme: theme::Theme,
) -> Result<(Vec<String>, String), CodegenError> {
    let mut emitter = ShaderEmitter::metal_with_theme(HashMap::new(), theme);
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

    /// Generated fragments are only half a module: they are written against
    /// the shared widget preamble's `WidgetVaryings` and are compiled
    /// concatenated onto it, so validate the same assembly the backend builds.
    fn assert_valid_wgsl(fragment_source: &str) {
        let assembled = crate::ui::wgsl_shaders::widget_shader_module(None, fragment_source);
        let source = assembled.as_str();
        let module = naga::front::wgsl::parse_str(source).unwrap_or_else(|error| {
            panic!(
                "WGSL parse failed:\n{}\n\n{}",
                error.emit_to_string(source),
                source
            )
        });
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("WGSL validation failed: {error:#?}\n\n{source}"));
    }

    fn expression_source(expr: &Expression) -> String {
        match expr {
            Expression::Symbol(value) => value.clone(),
            Expression::Keyword(value) => format!(":{value}"),
            Expression::String(value) => format!("{value:?}"),
            Expression::Number(value) => format_float(*value),
            Expression::List(items) => format!(
                "({})",
                items
                    .iter()
                    .map(expression_source)
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            Expression::QuoteSymbol(value) => format!("'{value}"),
            Expression::QuoteList(items) => format!(
                "'({})",
                items
                    .iter()
                    .map(expression_source)
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            Expression::Quasiquote(value) => format!("`{}", expression_source(value)),
            Expression::Unquote(value) => format!(",{}", expression_source(value)),
            Expression::UnquoteSplicing(value) => format!(",@{}", expression_source(value)),
        }
    }

    fn shader_from_defwidget(expr: &Expression) -> Option<(&Expression, Vec<String>)> {
        let Expression::List(items) = expr else {
            return None;
        };
        if !matches!(items.first(), Some(Expression::Symbol(head)) if head == "defwidget") {
            return None;
        }

        let mut shader = None;
        let mut state = Vec::new();
        let mut index = 2;
        while index + 1 < items.len() {
            if let Expression::Keyword(key) = &items[index] {
                match key.as_str() {
                    "shader" => shader = Some(&items[index + 1]),
                    "state" => {
                        if let Expression::List(symbols) = &items[index + 1] {
                            state.extend(symbols.iter().filter_map(|symbol| match symbol {
                                Expression::Symbol(name) => Some(name.clone()),
                                _ => None,
                            }));
                        }
                    }
                    _ => {}
                }
                index += 2;
            } else {
                index += 1;
            }
        }
        shader.map(|shader| (shader, state))
    }

    fn content_lisp_files(path: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                content_lisp_files(&path, out);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "lisp")
                && std::fs::read_to_string(&path).is_ok_and(|source| source.contains(":shader"))
            {
                out.push(path);
            }
        }
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
    fn content_shader_corpus_emits_valid_wgsl() {
        use crate::runtime::Runtime;

        let content = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let mut files = Vec::new();
        content_lisp_files(&content, &mut files);
        files.sort_by_key(|path| {
            let is_materials = path.ends_with("ui/materials.lisp");
            (!is_materials, path.clone())
        });

        use sha2::{Digest, Sha256};

        let mut runtime = Runtime::new();
        let mut metal_snapshot = Sha256::new();
        let mut wgsl_snapshot = Sha256::new();
        let mut shader_count = 0;
        for path in files {
            let source = std::fs::read_to_string(&path).unwrap();
            let tokens = crate::parser::Parser::new(source).parse().unwrap();
            let expressions = crate::parser::ASTParser::new(tokens).parse().unwrap();
            let mut module_name = None;
            for expression in expressions {
                if let Expression::List(items) = &expression
                    && matches!(items.first(), Some(Expression::Symbol(head)) if head == "module")
                    && let Some(Expression::Symbol(name)) = items.get(1)
                {
                    module_name = Some(name.clone());
                    continue;
                }
                if let Expression::List(items) = &expression
                    && matches!(items.first(), Some(Expression::Symbol(head)) if head == "defmacro")
                {
                    let mut qualified = items.clone();
                    if let (Some(module), Some(Expression::Symbol(name))) =
                        (module_name.as_ref(), qualified.get_mut(1))
                    {
                        *name = format!("{module}/{name}");
                    }
                    let qualified = Expression::List(qualified);
                    runtime
                        .eval_str(&expression_source(&qualified))
                        .unwrap_or_else(|error| {
                            panic!(
                                "failed to register corpus macro from {}: {error:?}",
                                path.display()
                            )
                        });
                    continue;
                }

                let Some((shader, state_symbols)) = shader_from_defwidget(&expression) else {
                    continue;
                };
                let expanded = runtime
                    .expand_macros_expression(shader)
                    .unwrap_or_else(|error| {
                        panic!("failed to expand shader from {}: {error}", path.display())
                    });
                let state_bindings = state_symbols.into_iter().collect::<HashSet<_>>();
                let mut state_symbols = collect_state_symbols(&expanded, &state_bindings);
                state_symbols.truncate(crate::widget_render::sdf_widget::MAX_SDF_STATE_UNIFORMS);
                let metal = compile_sdf_to_metal_with_state_and_theme(
                    &expanded,
                    &state_symbols,
                    theme::default_theme(),
                )
                .unwrap_or_else(|error| {
                    panic!("MSL codegen failed for {}: {error}", path.display())
                });
                let wgsl = compile_sdf_to_wgsl_with_state_and_theme(
                    &expanded,
                    &state_symbols,
                    theme::default_theme(),
                )
                .unwrap_or_else(|error| {
                    panic!("WGSL codegen failed for {}: {error}", path.display())
                });
                assert_eq!(metal.region_count, wgsl.region_count, "{}", path.display());
                metal_snapshot.update(metal.shader_source.as_bytes());
                metal_snapshot.update([0]);
                wgsl_snapshot.update(wgsl.shader_source.as_bytes());
                wgsl_snapshot.update([0]);
                assert_valid_wgsl(&wgsl.shader_source);
                shader_count += 1;
            }
        }
        assert!(
            shader_count >= 60,
            "expected the full content shader corpus"
        );
        let metal_snapshot = metal_snapshot
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        // Re-captured after 867fb9a9, which finished the restyle that started at
        // 7396708d: disclosure glyphs became SDF symbols (new `triangle` and
        // `disclosure-*` macros in core/sdf-stdlib.lisp, the `disclosure-button`
        // widget in ui/materials.lisp), ui/transport.lisp icon bodies and theme
        // keywords moved, and ui/sequencer.lisp lighting `vec3` z values changed.
        // Both digests cover the authored corpus, so any deliberate content
        // shader edit moves them; nothing in the emitters changed here.
        assert_eq!(
            metal_snapshot,
            "23b6d51596046ab185bcb7d2cc6b749e5345ded16cbd5e33402b320d1bf32358"
        );
        let wgsl_snapshot = wgsl_snapshot
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        // Captured in eseq-linux.7 when the emitter stopped emitting its own
        // `WidgetVaryings` (the struct now comes from the shared WGSL widget
        // preamble the fragments are assembled onto), and re-captured alongside
        // the MSL digest above for the same content edits.
        assert_eq!(
            wgsl_snapshot,
            "1a8426d0783e7ad53ee9177d072ce187aa50c19365afdcadbab7c633fd90ae78"
        );
    }

    #[test]
    fn wgsl_distance_shader_is_valid() {
        let output = compile_sdf_to_wgsl(&parse_one_expr(
            "(let ((wrapped (mod (+ value_t 1) 1))
                   (positive (> x 0)))
               (- (length (vec2 x y)) (if positive wrapped 0.5)))",
        ))
        .unwrap();
        assert_valid_wgsl(&output.shader_source);
        assert!(output.shader_source.contains("@fragment"));
        assert!(output.shader_source.contains("vec2<f32>(x, y)"));
        assert!(output.shader_source.contains("%"));
        assert!(output.shader_source.contains(": bool ="));
    }

    #[test]
    fn flat_lighting_quality_eliminates_offset_field_samples_in_both_emitters() {
        let expr = parse_one_expr(
            "(sdf/layer
               (sdf/fill (sin (+ (* x 7) (* y 11)))
                 (material
                   :lighting (lighting :edge-min -0.2
                                       :edge-max 0.2
                                       :light (vec3 0.2 -0.4 1)
                                       :shininess 32)
                   :color (rgba (+ 0.5 (* 0.5 diffuse)) specular
                                (dot normal (vec3 0 0 1)) 1))))",
        );
        let full = compile_sdf_to_wgsl(&expr).unwrap();
        let flat_options = SdfShaderOptions::flat_lighting();
        let flat_wgsl = compile_sdf_to_wgsl_with_options(&expr, flat_options).unwrap();
        let flat_metal = compile_sdf_to_metal_with_options(&expr, flat_options).unwrap();

        assert_eq!(full.shader_source.matches("sin(").count(), 5);
        assert_eq!(flat_wgsl.shader_source.matches("sin(").count(), 1);
        assert_eq!(flat_metal.shader_source.matches("sin(").count(), 1);
        assert!(!flat_wgsl.shader_source.contains("normalize("));
        assert!(!flat_wgsl.shader_source.contains("pow("));
        assert!(flat_wgsl.shader_source.contains("vec3<f32>(0.0, 0.0, 1.0)"));
        assert!(flat_metal.shader_source.contains("float3(0.0, 0.0, 1.0)"));
        assert_valid_wgsl(&flat_wgsl.shader_source);
    }

    #[test]
    fn lighting_quality_parser_rejects_unknown_tiers() {
        assert_eq!(SdfLightingQuality::parse("full"), Ok(SdfLightingQuality::Full));
        assert_eq!(SdfLightingQuality::parse("flat"), Ok(SdfLightingQuality::Flat));
        let error = SdfLightingQuality::parse("fast").unwrap_err();
        assert!(error.contains("expected 'full' or 'flat'"));
    }

    #[test]
    fn wgsl_color_shader_with_material_features_and_state_is_valid() {
        let expr = parse_one_expr(
            "(sdf/layer
               (sdf/fill (- (length (vec2 x y)) radius)
                 (material
                   :color (if hit/hover :accent (rgba diffuse specular 0.25 1))
                   :shadow (shadow :color (rgba 0 0 0 0.2)
                                   :blur 0.18
                                   :offset (vec2 0 0.05))
                   :lighting (lighting :edge-min -0.2
                                       :edge-max 0.2
                                       :light (vec3 0.2 -0.4 1)
                                       :shininess 32))))",
        );
        let output = compile_sdf_to_wgsl_with_state(&expr, &[String::from("radius")]).unwrap();
        assert_eq!(output.region_count, 1);
        assert_valid_wgsl(&output.shader_source);
        assert!(
            output
                .shader_source
                .contains("let sdf_state_radius: f32 = input.uniform_a.x;")
        );
        assert!(output.shader_source.contains("select("));
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
        use crate::runtime::Runtime;

        let mut rt = Runtime::new();
        rt.expand_macros_expression(&parse_one_expr(src))
            .expect("expand SDF macro")
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
        let theme = theme::current();
        let legacy = compile_sdf_expr_with_theme(
            &parse_one_expr("(sdf/fill (- (length (vec2 x y)) 0.5) :accent)"),
            theme,
        )
        .unwrap();
        let material = compile_sdf_expr_with_theme(
            &parse_one_expr("(sdf/fill (- (length (vec2 x y)) 0.5) (material :color :accent))"),
            theme,
        )
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
    fn let_infers_float4_for_mix_of_color_bindings() {
        let (stmts, _result) = codegen_expr(
            "(let ((track-color (mix :dim :primary 0.25))
                   (arc-color (mix track-color :red 0.5)))
               arc-color)",
        );
        let all = stmts.join("\n");
        assert!(all.contains("float4"));
        assert!(!all.contains("float _v0 = ((float4"));
        assert!(!all.contains("float _v1 = ((_v0) + (((float4"));
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
        let mut emitter = ShaderEmitter::metal(HashMap::new());
        let expr = parse_one_expr("(sdf/fill (- (length (vec2 x y)) 0.5) :accent)");
        let _ = emitter.emit_expr(&expr).unwrap();
        assert_eq!(emitter.next_region_id, 1);
    }

    #[test]
    fn sdf_paint_no_region() {
        let mut emitter = ShaderEmitter::metal(HashMap::new());
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
        let mut emitter = ShaderEmitter::metal(HashMap::new());
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
    fn captured_state_symbols_use_extended_uniform_slots() {
        let expr = expand_sdf("(sdf/layer (sdf/fill (sdf/circle s15) :accent))");
        let symbols = (0..16).map(|idx| format!("s{idx}")).collect::<Vec<_>>();
        let output = compile_sdf_to_metal_with_state(&expr, &symbols).unwrap();
        assert!(
            output
                .shader_source
                .contains("float sdf_state_s8 = in.uniform_c.x;")
        );
        assert!(
            output
                .shader_source
                .contains("float sdf_state_s15 = in.uniform_d.w;")
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
