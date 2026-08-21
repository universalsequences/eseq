use crate::parser::Expression;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Chunk {
    pub ops: Vec<OpCode>,
    pub constants: Vec<f64>,
    pub strings: Vec<String>, // string constants pool
    pub symbols: Vec<String>,
    pub upvalues: Vec<String>,
    pub source_symbol: Option<String>,
    pub source_file: Option<PathBuf>,
    /// The module the chunk's code was compiled under (spec §5): `None`
    /// for the implicit `eseq.vanilla`. Lets runtime registration forms
    /// (widget `:key`, `bind-key`, `define-mode`, …) know the module
    /// current at the executing call site.
    pub source_module: Option<String>,
}

#[derive(Debug)]
enum SymbolResolution {
    Global(usize),
    Upvalue(usize),
    Local(usize),
}

struct Scope {
    pub chunk_idx: usize,
    pub symbols: Vec<String>,
    pub upvalues: Vec<String>,
}

pub enum CompilerError {
    UnknownOperator,
    InvalidArg,
    /// A resolution error with a human-readable message (unknown import
    /// alias/namespace, malformed `module`/`import` forms).
    Message(String),
}

#[derive(Debug, Clone)]
pub enum OpCode {
    Push,
    PushConst(usize), // const idx
    PushStr(usize),   // string const idx
    Dup,
    Pop,
    Load(usize),
    LoadGlobal(usize),
    LoadLocal(usize),
    LoadUpvalue(usize),
    StoreGlobal(usize),
    StoreLocal(usize),
    StoreUpvalue(usize),
    StoreState(u32),
    StoreField(usize),
    Store(usize),
    Add(usize),
    Mul(usize),
    Sub(usize),
    Div(usize),
    Min(usize),
    Max(usize),
    Eq,
    Lt,
    Gt,
    Lte,
    Gte,
    MakeList(usize),
    Call(usize),
    MakeFunc(usize),
    MakeClosure(usize, usize),
    Eval,                    // pop a string, eval it in the current VM context, push result
    PushKeyword(usize),      // push Value::Keyword from strings pool
    PushSymbol(usize),       // push Value::Symbol from strings pool (quoted symbol)
    InitDerived(u32, usize), // node id, chunk idx
    InitEffect(u32, usize),  // node id, chunk idx
    InitNamedEffect(u32, usize, usize), // node id, chunk idx, target buffer name string idx
    InitState(u32),          // node id
    LoadDerived(u32),        // load derived node cached value
    LoadState(u32),          // load state node current value
    DerivedBegin(u32),
    DerivedEnd(u32),
    EffectBegin(u32),
    EffectEnd(u32),
    SubtreeBegin,
    SubtreeEnd,
    LoadReactive(usize, usize),    // namespace idx, field idx
    LoadReactiveNth(usize, usize), // namespace idx, field idx; pops list index from stack
    LoadReactiveLen(usize, usize), // namespace idx, field idx; pushes list length
    StoreReactive(usize, usize),   // namespace idx, field idx
    GetField(usize),               // pop a map, push map[strings[idx]]
    EmitTree,                      // pop widget tree from stack and route it to the runtime
    Return,
    Jump(usize),
    JumpIfFalse(usize),
    PushBool(bool),
    PushNil,
}

#[derive(Debug, Clone)]
pub struct MacroDef {
    pub params: Vec<String>,
    pub rest_param: Option<String>,
    pub body: Expression,
}

/// Compiler-local module state carried across the segment compilers of one
/// compile unit (see `Compiler::take_module_context`).
#[derive(Debug, Clone)]
pub struct ModuleCompileContext {
    current_module: String,
    module_declared: bool,
    import_aliases: HashMap<String, String>,
    refers: HashMap<String, String>,
    known_namespaces: HashSet<String>,
}

pub struct Compiler {
    expressions: Vec<Expression>,
    chunks: Vec<Chunk>,
    scopes: Vec<Scope>,
    current_chunk: usize,
    global_symbols: Vec<String>,
    reactive_namespaces: HashSet<String>,
    derived_bindings: HashMap<String, u32>,
    state_bindings: HashMap<String, u32>,
    next_node_id: u32,
    next_temp_id: u32,
    source_file: Option<PathBuf>,
    /// The module bare global names qualify under: the implicit
    /// `eseq.vanilla` until a `(module …)` header switches it (spec §2).
    current_module: String,
    /// Whether this compile unit has seen a `(module …)` form (one per
    /// file, spec §2 decision 1).
    module_declared: bool,
    /// `(import NAME :as ALIAS)` bindings, per compile unit (spec §2
    /// decision 2): alias → full module name.
    import_aliases: HashMap<String, String>,
    /// `(import NAME :refer (sym …))` bindings: bare symbol → qualified
    /// name it resolves to (spec §3 resolution order).
    refers: HashMap<String, String>,
    /// Namespaces made known by qualified `def` targets in this unit (the
    /// §3 escape hatch may define into a module before it exists).
    known_namespaces: std::collections::HashSet<String>,
    /// Export visibility of modules already evaluated by this VM.
    module_exports: super::modules::ModuleExportRegistry,
    /// Non-fatal compile diagnostics (visibility references, escape-hatch
    /// defs). Drained by the VM into the diagnostics channel.
    warnings: Vec<String>,
    /// Fatal resolution errors (unknown alias/namespace, malformed module
    /// forms) recorded mid-compile and reported when `compile` finishes.
    errors: Vec<String>,
    pub macros: HashMap<String, MacroDef>,
}

fn is_widget_name(name: &str) -> bool {
    matches!(
        name,
        "label"
            | "button"
            | "badge"
            | "slider"
            | "hslider"
            | "vslider"
            | "toggle"
            | "piano-keyboard"
            | "linegraph"
            | "matrix"
            | "knob"
            | "meter"
            | "modulator-curve"
            | "text-input"
            | "select"
            | "v-stack"
            | "h-stack"
            | "wrap"
            | "box"
            | "grid"
            | "tabs"
            | "patcher"
            | "response-curve-editor"
            | "eq8-editor"
            | "scroll"
            | "tree"
    )
}

fn extract_function_definition(
    list: &[Expression],
) -> Option<(Option<String>, Vec<Expression>, Vec<Expression>)> {
    match (list.first(), list.get(1), list.get(2)) {
        (
            Some(Expression::Symbol(s)),
            Some(Expression::Symbol(name)),
            Some(Expression::List(args)),
        ) if s == "def" && list.len() >= 4 => {
            Some((Some(name.to_string()), args.clone(), list[3..].to_vec()))
        }
        (Some(Expression::Symbol(s)), Some(Expression::List(args)), _)
            if s == "lambda" && list.len() >= 3 =>
        {
            Some((None, args.clone(), list[2..].to_vec()))
        }
        _ => None,
    }
}

fn extract_if_statement(list: &[Expression]) -> Option<(Expression, Expression, Expression)> {
    match (list.first(), list.get(1), list.get(2), list.get(3)) {
        (Some(Expression::Symbol(s)), Some(condition), Some(then_body), Some(else_body))
            if s == "if" =>
        {
            Some((condition.clone(), then_body.clone(), else_body.clone()))
        }
        (Some(Expression::Symbol(s)), Some(condition), Some(then_body), None) if s == "if" => {
            Some((
                condition.clone(),
                then_body.clone(),
                Expression::Symbol("nil".to_string()),
            ))
        }
        _ => None,
    }
}

fn extract_match_statement(
    list: &[Expression],
) -> Option<(
    Expression,
    Vec<(Expression, Expression)>,
    Option<Expression>,
)> {
    let (Some(Expression::Symbol(s)), Some(value)) = (list.first(), list.get(1)) else {
        return None;
    };
    if s != "match" || list.len() < 4 {
        return None;
    }

    let mut branches = Vec::new();
    let mut default = None;
    let mut idx = 2;
    while idx + 1 < list.len() {
        let pattern = list[idx].clone();
        let body = list[idx + 1].clone();
        if matches!(&pattern, Expression::Symbol(sym) if sym == "_") {
            default = Some(body);
            break;
        }
        branches.push((pattern, body));
        idx += 2;
    }

    Some((value.clone(), branches, default))
}

fn extract_boolean_chain<'a>(list: &'a [Expression], name: &str) -> Option<&'a [Expression]> {
    let Some(Expression::Symbol(symbol)) = list.first() else {
        return None;
    };
    (symbol == name).then_some(&list[1..])
}

fn extract_zip_sources(source: &Expression) -> Option<Vec<Expression>> {
    let Expression::List(items) = source else {
        return None;
    };
    let Some(Expression::Symbol(name)) = items.first() else {
        return None;
    };
    (name == "zip").then(|| items[1..].to_vec())
}

impl Compiler {
    pub fn new(expressions: Vec<Expression>) -> Self {
        Compiler {
            expressions,
            chunks: vec![],
            scopes: vec![],
            current_chunk: 0,
            global_symbols: vec![],
            reactive_namespaces: HashSet::new(),
            derived_bindings: HashMap::new(),
            state_bindings: HashMap::new(),
            next_node_id: 0,
            next_temp_id: 0,
            source_file: None,
            current_module: super::modules::IMPLICIT_MODULE.to_string(),
            module_declared: false,
            import_aliases: HashMap::new(),
            refers: HashMap::new(),
            known_namespaces: std::collections::HashSet::new(),
            module_exports: super::modules::ModuleExportRegistry::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
            macros: HashMap::new(),
        }
    }

    /// For REPL/eval_str: start with existing chunks and global symbol table
    /// so new code compiles against the same indices.
    pub fn new_repl(
        expressions: Vec<Expression>,
        existing_chunks: Vec<Chunk>,
        existing_global_names: Vec<String>,
        reactive_namespaces: HashSet<String>,
        derived_bindings: HashMap<String, u32>,
        state_bindings: HashMap<String, u32>,
        next_node_id: u32,
        macros: HashMap<String, MacroDef>,
        source_file: Option<PathBuf>,
    ) -> Self {
        Compiler {
            expressions,
            chunks: existing_chunks,
            scopes: vec![],
            current_chunk: 0,
            global_symbols: existing_global_names,
            reactive_namespaces,
            derived_bindings,
            state_bindings,
            next_node_id,
            next_temp_id: 0,
            source_file,
            current_module: super::modules::IMPLICIT_MODULE.to_string(),
            module_declared: false,
            import_aliases: HashMap::new(),
            refers: HashMap::new(),
            known_namespaces: std::collections::HashSet::new(),
            module_exports: super::modules::ModuleExportRegistry::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
            macros,
        }
    }

    /// The compiler-local module state a compile unit accumulates as its
    /// top-level forms are walked: the `(module …)` declaration, `:as` /
    /// `:refer` bindings and the namespaces made known so far. When the VM
    /// splits a unit at top-level `import` boundaries (spec §4: import's
    /// compile-time half) each continuation segment gets a fresh compiler
    /// re-seeded from the VM, and this context is what threads the unit's
    /// module identity across those segment compilers.
    pub fn take_module_context(&mut self) -> ModuleCompileContext {
        ModuleCompileContext {
            current_module: std::mem::replace(
                &mut self.current_module,
                super::modules::IMPLICIT_MODULE.to_string(),
            ),
            module_declared: self.module_declared,
            import_aliases: std::mem::take(&mut self.import_aliases),
            refers: std::mem::take(&mut self.refers),
            known_namespaces: std::mem::take(&mut self.known_namespaces),
        }
    }

    pub fn set_module_context(&mut self, context: ModuleCompileContext) {
        self.current_module = context.current_module;
        self.module_declared = context.module_declared;
        self.import_aliases = context.import_aliases;
        self.refers = context.refers;
        self.known_namespaces = context.known_namespaces;
    }

    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    pub fn set_module_exports(&mut self, exports: super::modules::ModuleExportRegistry) {
        self.module_exports = exports;
    }

    /// Validate `:refer` contracts after the import segment has executed and
    /// therefore made the imported module's export set available.
    pub fn validate_refers(
        &self,
        exports: &super::modules::ModuleExportRegistry,
    ) -> Result<(), String> {
        for (bare, qualified) in &self.refers {
            let Some((module, name)) = super::modules::split_qualified(qualified) else {
                continue;
            };
            if super::modules::exported_from(exports, module, name) == Some(false) {
                return Err(format!(
                    "import {module}: cannot :refer non-exported symbol '{bare}'"
                ));
            }
        }
        Ok(())
    }

    /// Whether a loaded module keeps `base` to itself. A module this unit has
    /// not seen loaded has no export set to consult (export spec §3 load-order
    /// interaction), so visibility checking is skipped.
    fn hidden_by(&self, module: &str, base: &str) -> bool {
        super::modules::exported_from(&self.module_exports, module, base)
            .is_some_and(|exported| !exported)
    }

    pub fn macros(&self) -> &HashMap<String, MacroDef> {
        &self.macros
    }

    fn alloc_temp_symbol(&mut self, prefix: &str) -> String {
        let symbol = format!("__eseq_{}_{}", prefix, self.next_temp_id);
        self.next_temp_id += 1;
        symbol
    }

    /// Namespace-aware macro lookup (spec §3): qualified names rewrite
    /// through import aliases then hit the table exactly (with a flat
    /// fallback for legacy hand-rolled keys); bare names prefer the
    /// current module, then `:refer`s, then the flat table.
    pub fn lookup_macro(&self, name: &str) -> Option<&MacroDef> {
        if let Some((ns, base)) = super::modules::split_qualified(name) {
            let full_ns = self
                .import_aliases
                .get(ns)
                .map(String::as_str)
                .unwrap_or(ns);
            return self
                .macros
                .get(&super::modules::qualify(full_ns, base))
                .or_else(|| self.macros.get(name));
        }
        self.macros
            .get(&super::modules::qualify(&self.current_module, name))
            .or_else(|| {
                self.refers
                    .get(name)
                    .and_then(|qualified| self.macros.get(qualified))
            })
            .or_else(|| self.macros.get(name))
    }

    pub fn expand_macros(&self, expr: &Expression, depth: usize) -> Expression {
        if depth > 100 {
            return expr.clone();
        }
        match expr {
            Expression::List(items) if !items.is_empty() => {
                if let Expression::Symbol(name) = &items[0] {
                    if let Some(mac) = self.lookup_macro(name) {
                        let args = &items[1..];
                        let arity_matches = match &mac.rest_param {
                            Some(_) => args.len() >= mac.params.len(),
                            None => args.len() == mac.params.len(),
                        };
                        if arity_matches {
                            // Build parameter bindings: expand macro args first.
                            let mut bindings = HashMap::new();
                            for (param, arg) in mac.params.iter().zip(args) {
                                bindings.insert(param.clone(), self.expand_macros(arg, depth + 1));
                            }
                            if let Some(rest_param) = &mac.rest_param {
                                bindings.insert(
                                    rest_param.clone(),
                                    Expression::List(
                                        args[mac.params.len()..]
                                            .iter()
                                            .map(|arg| self.expand_macros(arg, depth + 1))
                                            .collect(),
                                    ),
                                );
                            }
                            if let Some(expanded) = Self::expand_quasiquote(&mac.body, &bindings) {
                                return self.expand_macros(&expanded, depth + 1);
                            }
                        }
                    }
                }
                // Not a macro call — recursively expand children
                Expression::List(
                    items
                        .iter()
                        .map(|item| self.expand_macros(item, depth + 1))
                        .collect(),
                )
            }
            _ => expr.clone(),
        }
    }

    fn expand_quasiquote(
        expr: &Expression,
        bindings: &HashMap<String, Expression>,
    ) -> Option<Expression> {
        match expr {
            Expression::Quasiquote(inner) => Self::expand_quasiquote_inner(inner, bindings),
            // If the macro body isn't quasiquoted, just return it as-is.
            _ => Some(expr.clone()),
        }
    }

    fn expand_quasiquote_inner(
        expr: &Expression,
        bindings: &HashMap<String, Expression>,
    ) -> Option<Expression> {
        match expr {
            Expression::Unquote(inner) => {
                // Substitute if the unquoted expression is a bound parameter.
                if let Expression::Symbol(name) = inner.as_ref() {
                    if let Some(replacement) = bindings.get(name) {
                        return Some(replacement.clone());
                    }
                }
                // Not a bound parameter — return inner as-is.
                Some(*inner.clone())
            }
            Expression::UnquoteSplicing(_) => None,
            Expression::List(items) | Expression::QuoteList(items) => {
                let mut expanded = Vec::with_capacity(items.len());
                for item in items {
                    if let Expression::UnquoteSplicing(inner) = item {
                        let Expression::Symbol(name) = inner.as_ref() else {
                            return None;
                        };
                        let Some(replacement) = bindings.get(name) else {
                            return None;
                        };
                        match replacement {
                            Expression::List(values) | Expression::QuoteList(values) => {
                                expanded.extend(values.iter().cloned());
                            }
                            _ => return None,
                        }
                    } else {
                        expanded.push(Self::expand_quasiquote_inner(item, bindings)?);
                    }
                }
                Some(if matches!(expr, Expression::QuoteList(_)) {
                    Expression::QuoteList(expanded)
                } else {
                    Expression::List(expanded)
                })
            }
            // Everything else inside quasiquote is literal.
            _ => Some(expr.clone()),
        }
    }

    fn desugar_pattern_binding(
        &mut self,
        pattern: &Expression,
        value: Expression,
        inner: Expression,
    ) -> Result<Expression, CompilerError> {
        match pattern {
            Expression::Symbol(_) => Ok(Expression::List(vec![
                Expression::List(vec![
                    Expression::Symbol("lambda".to_string()),
                    Expression::List(vec![pattern.clone()]),
                    inner,
                ]),
                value,
            ])),
            Expression::List(fields) => {
                let temp_symbol = self.alloc_temp_symbol("destructure");
                let mut nested = inner;
                for field in fields.iter().rev() {
                    let Expression::Symbol(name) = field else {
                        return Err(CompilerError::InvalidArg);
                    };
                    nested = self.desugar_pattern_binding(
                        &Expression::Symbol(name.clone()),
                        Expression::List(vec![
                            Expression::Symbol("get".to_string()),
                            Expression::Symbol(temp_symbol.clone()),
                            Expression::Keyword(name.clone()),
                        ]),
                        nested,
                    )?;
                }
                Ok(Expression::List(vec![
                    Expression::List(vec![
                        Expression::Symbol("lambda".to_string()),
                        Expression::List(vec![Expression::Symbol(temp_symbol)]),
                        nested,
                    ]),
                    value,
                ]))
            }
            _ => Err(CompilerError::InvalidArg),
        }
    }

    fn desugar_threading(
        &self,
        initial: &Expression,
        stages: &[Expression],
        thread_last: bool,
    ) -> Result<Expression, CompilerError> {
        stages
            .iter()
            .try_fold(initial.clone(), |acc, stage| match stage {
                Expression::Symbol(name) => Ok(Expression::List(vec![
                    Expression::Symbol(name.clone()),
                    acc,
                ])),
                Expression::List(items) if !items.is_empty() => {
                    let mut rewritten = items.clone();
                    let insert_idx = if thread_last { rewritten.len() } else { 1 };
                    rewritten.insert(insert_idx, acc);
                    Ok(Expression::List(rewritten))
                }
                _ => Err(CompilerError::InvalidArg),
            })
    }

    fn desugar_and(&mut self, exprs: &[Expression]) -> Result<Expression, CompilerError> {
        if exprs.is_empty() {
            return Ok(Expression::Symbol("true".to_string()));
        }
        if exprs.len() == 1 {
            return Ok(exprs[0].clone());
        }

        let first = exprs[0].clone();
        let temp = self.alloc_temp_symbol("and");
        let rest = self.desugar_and(&exprs[1..])?;
        Ok(Expression::List(vec![
            Expression::Symbol("let".to_string()),
            Expression::List(vec![Expression::List(vec![
                Expression::Symbol(temp.clone()),
                first,
            ])]),
            Expression::List(vec![
                Expression::Symbol("if".to_string()),
                Expression::Symbol(temp.clone()),
                rest,
                Expression::Symbol(temp),
            ]),
        ]))
    }

    fn desugar_or(&mut self, exprs: &[Expression]) -> Result<Expression, CompilerError> {
        if exprs.is_empty() {
            return Ok(Expression::Symbol("nil".to_string()));
        }
        if exprs.len() == 1 {
            return Ok(exprs[0].clone());
        }

        let first = exprs[0].clone();
        let temp = self.alloc_temp_symbol("or");
        let rest = self.desugar_or(&exprs[1..])?;
        Ok(Expression::List(vec![
            Expression::Symbol("let".to_string()),
            Expression::List(vec![Expression::List(vec![
                Expression::Symbol(temp.clone()),
                first,
            ])]),
            Expression::List(vec![
                Expression::Symbol("if".to_string()),
                Expression::Symbol(temp.clone()),
                Expression::Symbol(temp),
                rest,
            ]),
        ]))
    }

    fn compile_quoted_expression(&mut self, expression: &Expression) -> Result<(), CompilerError> {
        match expression {
            Expression::List(items) | Expression::QuoteList(items) => {
                for item in items {
                    self.compile_quoted_expression(item)?;
                }
                self.emit(OpCode::MakeList(items.len()));
            }
            Expression::Symbol(s) | Expression::QuoteSymbol(s) => {
                let idx = self.use_string_constant(s);
                self.emit(OpCode::PushSymbol(idx));
            }
            Expression::Keyword(s) => {
                let idx = self.use_string_constant(s);
                self.emit(OpCode::PushKeyword(idx));
            }
            Expression::Number(n) => {
                let constant_idx = self.use_constant(*n);
                self.emit(OpCode::PushConst(constant_idx));
            }
            Expression::String(s) => {
                let str_idx = self.use_string_constant(s);
                self.emit(OpCode::PushStr(str_idx));
            }
            Expression::Quasiquote(inner) => {
                self.compile_quoted_expression(inner)?;
            }
            Expression::Unquote(inner) | Expression::UnquoteSplicing(inner) => {
                self.compile_quoted_expression(inner)?;
            }
        }

        Ok(())
    }

    /// Like `compile_quoted_expression`, but an inner `'x` / `'(...)` is captured
    /// as `(quote x)` data instead of being flattened into plain data. Used only
    /// for `def-sequencer` `:tick`/`:init` bodies, whose captured data is
    /// re-serialized to source (`format_lisp_source` prints the wrapper back as
    /// `'x`) and re-evaluated in the scheduler VM — without the wrapper, quotes
    /// authored inside a tick body would silently become live call forms there.
    fn compile_quoted_expression_preserving_quotes(
        &mut self,
        expression: &Expression,
    ) -> Result<(), CompilerError> {
        match expression {
            Expression::QuoteList(items) => {
                let quote_idx = self.use_string_constant("quote");
                self.emit(OpCode::PushSymbol(quote_idx));
                for item in items {
                    self.compile_quoted_expression_preserving_quotes(item)?;
                }
                self.emit(OpCode::MakeList(items.len()));
                self.emit(OpCode::MakeList(2));
            }
            Expression::QuoteSymbol(s) => {
                let quote_idx = self.use_string_constant("quote");
                self.emit(OpCode::PushSymbol(quote_idx));
                let idx = self.use_string_constant(s);
                self.emit(OpCode::PushSymbol(idx));
                self.emit(OpCode::MakeList(2));
            }
            Expression::List(items) => {
                for item in items {
                    self.compile_quoted_expression_preserving_quotes(item)?;
                }
                self.emit(OpCode::MakeList(items.len()));
            }
            other => self.compile_quoted_expression(other)?,
        }

        Ok(())
    }

    /// Consume the compiler and return the final global symbol table,
    /// so the VM can sync its own name→index mapping.
    pub fn into_global_names(self) -> Vec<String> {
        self.global_symbols
    }

    pub fn global_names(&self) -> Vec<String> {
        self.global_symbols.clone()
    }

    /// Move the compiled program state out of the compiler. The REPL path
    /// (`Vm::eval_str`) runs per keystroke on shortcut dispatch, so it must
    /// not deep-clone thousands of chunks/names/macros in either direction.
    pub fn take_chunks(&mut self) -> Vec<Chunk> {
        std::mem::take(&mut self.chunks)
    }

    pub fn take_global_names(&mut self) -> Vec<String> {
        std::mem::take(&mut self.global_symbols)
    }

    pub fn take_derived_bindings(&mut self) -> HashMap<String, u32> {
        std::mem::take(&mut self.derived_bindings)
    }

    pub fn take_state_bindings(&mut self) -> HashMap<String, u32> {
        std::mem::take(&mut self.state_bindings)
    }

    pub fn take_macros(&mut self) -> HashMap<String, MacroDef> {
        std::mem::take(&mut self.macros)
    }

    pub fn into_derived_bindings(self) -> HashMap<String, u32> {
        self.derived_bindings
    }

    pub fn derived_bindings(&self) -> HashMap<String, u32> {
        self.derived_bindings.clone()
    }

    pub fn state_bindings(&self) -> HashMap<String, u32> {
        self.state_bindings.clone()
    }

    pub fn next_node_id(&self) -> u32 {
        self.next_node_id
    }

    pub fn add_reactive_namespace(&mut self, name: String) {
        self.reactive_namespaces.insert(name);
    }

    fn alloc_node_id(&mut self) -> u32 {
        let id = self.next_node_id;
        self.next_node_id = self.next_node_id.saturating_add(1);
        id
    }

    fn compile_reactive_chunk(
        &mut self,
        node_id: u32,
        body: &[Expression],
        is_effect: bool,
    ) -> Result<usize, CompilerError> {
        let (chunk_idx, previous_chunk_idx) = self.new_chunk(Chunk {
            ops: vec![],
            constants: vec![],
            strings: vec![],
            symbols: vec![],
            upvalues: vec![],
            source_symbol: None,
            source_file: self.source_file.clone(),
            source_module: None,
        });

        if is_effect {
            self.emit(OpCode::EffectBegin(node_id));
        } else {
            self.emit(OpCode::DerivedBegin(node_id));
        }
        self.compile_block(body)?;
        if is_effect {
            self.emit(OpCode::EmitTree);
            self.emit(OpCode::EffectEnd(node_id));
            self.emit(OpCode::PushNil);
        } else {
            self.emit(OpCode::DerivedEnd(node_id));
        }
        self.emit(OpCode::Return);

        let _ = self.scopes.pop();
        self.current_chunk = previous_chunk_idx;
        Ok(chunk_idx)
    }

    fn compile_named_derived_definition(
        &mut self,
        name: &str,
        body: &[Expression],
    ) -> Result<(), CompilerError> {
        let node_id = self.alloc_node_id();
        let chunk_idx = self.compile_reactive_chunk(node_id, body, false)?;
        self.derived_bindings.insert(name.to_string(), node_id);
        self.emit(OpCode::InitDerived(node_id, chunk_idx));
        self.emit_symbol_store_for_definition(name);
        self.emit(OpCode::PushNil);
        Ok(())
    }

    fn compile_named_state_definition(
        &mut self,
        name: &str,
        initial: &Expression,
    ) -> Result<(), CompilerError> {
        // Registry auto-qualification (spec §5): a defstate inside a
        // declared module keys `state_bindings` qualified
        // (`eseq.mixer/panel-visible`); vanilla files keep flat keys so
        // serialized identity does not shift.
        let key = self.qualify_registration_name(name);
        let node_id = self
            .state_bindings
            .get(&key)
            .copied()
            .unwrap_or_else(|| self.alloc_node_id());
        self.state_bindings.insert(key, node_id);
        self.compile_expression(initial)?;
        self.emit(OpCode::InitState(node_id));
        let global_idx = self.use_global_for_definition(name);
        self.emit(OpCode::StoreGlobal(global_idx));
        self.emit(OpCode::PushNil);
        Ok(())
    }

    fn compile_custom_definition(&mut self, list: &[Expression]) -> Result<(), CompilerError> {
        let Expression::Symbol(name) = list.get(1).ok_or(CompilerError::InvalidArg)? else {
            return Err(CompilerError::InvalidArg);
        };
        let initial = list.get(2).ok_or(CompilerError::InvalidArg)?;
        let mut type_name = None;
        let mut doc = None;
        let mut index = 3;
        while index < list.len() {
            match list.get(index) {
                Some(Expression::Keyword(key)) if key == "type" => {
                    let value = list.get(index + 1).ok_or(CompilerError::InvalidArg)?;
                    type_name = match value {
                        Expression::Keyword(value) | Expression::Symbol(value) => Some(value.clone()),
                        _ => return Err(CompilerError::InvalidArg),
                    };
                    index += 2;
                }
                Some(Expression::Keyword(key)) if key == "doc" => {
                    let Expression::String(value) = list.get(index + 1).ok_or(CompilerError::InvalidArg)? else {
                        return Err(CompilerError::InvalidArg);
                    };
                    doc = Some(value.clone());
                    index += 2;
                }
                _ => return Err(CompilerError::Message(
                    "defcustom syntax is (defcustom name default :type TYPE :doc \"…\")".into()
                )),
            }
        }
        let type_name = type_name.ok_or_else(|| CompilerError::Message("defcustom requires :type".into()))?;
        let doc = doc.ok_or_else(|| CompilerError::Message("defcustom requires :doc".into()))?;

        let key = self.qualify_registration_name(name);
        let node_id = self.state_bindings.get(&key).copied().unwrap_or_else(|| self.alloc_node_id());
        self.state_bindings.insert(key, node_id);
        self.compile_expression(initial)?;
        self.emit(OpCode::Dup);
        self.emit(OpCode::InitState(node_id));
        let global_idx = self.use_global_for_definition(name);
        self.emit(OpCode::StoreGlobal(global_idx));
        for value in [name, type_name.as_str(), doc.as_str()] {
            let index = self.use_string_constant(value);
            self.emit(OpCode::PushStr(index));
        }
        self.emit_symbol_load("__register-defcustom");
        self.emit(OpCode::Call(4));
        Ok(())
    }

    fn compile_set_statement(
        &mut self,
        target: &Expression,
        value: &Expression,
    ) -> Result<(), CompilerError> {
        match target {
            Expression::Symbol(name) => {
                let parts = if super::modules::is_qualified(name) {
                    vec![name.as_str()]
                } else {
                    name.splitn(2, '.').collect::<Vec<_>>()
                };
                if parts.len() == 2 && self.reactive_namespaces.contains(parts[0]) {
                    self.compile_expression(value)?;
                    if parts[1].contains('.') {
                        return Err(CompilerError::InvalidArg);
                    }
                    let ns_idx = self.use_string_constant(parts[0]);
                    let field_idx = self.use_string_constant(parts[1]);
                    self.emit(OpCode::StoreReactive(ns_idx, field_idx));
                    self.emit(OpCode::LoadReactive(ns_idx, field_idx));
                    return Ok(());
                }
                if parts.len() == 2 {
                    let fields = parts[1].split('.').collect::<Vec<_>>();
                    self.emit_symbol_load(parts[0]);
                    for field in fields.iter().take(fields.len().saturating_sub(1)) {
                        let idx = self.use_string_constant(field);
                        self.emit(OpCode::GetField(idx));
                    }
                    self.compile_expression(value)?;
                    let last_idx =
                        self.use_string_constant(fields.last().copied().unwrap_or_default());
                    self.emit(OpCode::StoreField(last_idx));
                    return Ok(());
                }
                self.compile_expression(value)?;
                self.emit_symbol_store(name);
                self.emit_symbol_load(name);
                Ok(())
            }
            Expression::List(_) => Err(CompilerError::InvalidArg),
            Expression::Keyword(_) | Expression::String(_) | Expression::Number(_) => {
                Err(CompilerError::InvalidArg)
            }
            Expression::QuoteSymbol(_) | Expression::QuoteList(_) => Err(CompilerError::InvalidArg),
            Expression::Quasiquote(_)
            | Expression::Unquote(_)
            | Expression::UnquoteSplicing(_) => Err(CompilerError::InvalidArg),
        }?;

        Ok(())
    }

    fn compile_each_form(
        &mut self,
        source: &Expression,
        lambda: &Expression,
    ) -> Result<(), CompilerError> {
        self.compile_expression(source)?;
        match source {
            Expression::Symbol(path) => {
                let idx = self.use_string_constant(path);
                self.emit(OpCode::PushStr(idx));
            }
            _ => self.emit(OpCode::PushNil),
        }
        let lambda = self.rewrite_each_lambda(source, lambda)?;
        self.compile_expression(&lambda)?;
        self.emit_symbol_load("each");
        self.emit(OpCode::Call(3));
        Ok(())
    }

    fn rewrite_each_lambda(
        &mut self,
        source: &Expression,
        lambda: &Expression,
    ) -> Result<Expression, CompilerError> {
        let Expression::List(items) = lambda else {
            return Err(CompilerError::InvalidArg);
        };
        let Some((None, args, body)) = extract_function_definition(items) else {
            return Err(CompilerError::InvalidArg);
        };
        if !(1..=2).contains(&args.len()) {
            return Err(CompilerError::InvalidArg);
        }

        let (item_name, item_pattern, needs_destructure) = match &args[0] {
            Expression::Symbol(name) => (name.clone(), Expression::Symbol(name.clone()), false),
            Expression::List(pattern) => {
                let temp = self.alloc_temp_symbol("each_item");
                (temp.clone(), Expression::List(pattern.clone()), true)
            }
            _ => return Err(CompilerError::InvalidArg),
        };
        let index_name = if args.len() == 2 {
            match &args[1] {
                Expression::Symbol(name) => name.clone(),
                _ => return Err(CompilerError::InvalidArg),
            }
        } else {
            "__each_index".to_string()
        };

        let mut zip_bind_sources = HashMap::new();
        let positional_zip_bindings = if let (Expression::List(pattern), Some(zip_sources)) =
            (&args[0], extract_zip_sources(source))
        {
            if pattern.len() > zip_sources.len()
                || !pattern
                    .iter()
                    .all(|field| matches!(field, Expression::Symbol(_)))
            {
                return Err(CompilerError::InvalidArg);
            }

            let mut bindings = Vec::with_capacity(pattern.len());
            for (field, zip_source) in pattern.iter().zip(zip_sources.iter()) {
                let Expression::Symbol(name) = field else {
                    return Err(CompilerError::InvalidArg);
                };
                zip_bind_sources.insert(name.clone(), zip_source.clone());
                bindings.push(Expression::List(vec![
                    Expression::Symbol(name.clone()),
                    Expression::List(vec![
                        Expression::Symbol("nth".to_string()),
                        zip_source.clone(),
                        Expression::Symbol(index_name.clone()),
                    ]),
                ]));
            }
            Some(bindings)
        } else {
            None
        };

        let rewritten_body = body
            .iter()
            .map(|expr| {
                self.rewrite_each_bind_expr(
                    expr,
                    source,
                    &item_name,
                    &index_name,
                    &zip_bind_sources,
                )
            })
            .collect::<Vec<_>>();

        let mut lambda_args = vec![Expression::Symbol(item_name.clone())];
        if args.len() == 2 {
            lambda_args.push(Expression::Symbol(index_name.clone()));
        } else {
            lambda_args.push(Expression::Symbol(index_name.clone()));
        }

        let rewritten_body = if let Some(bindings) = positional_zip_bindings {
            vec![Expression::List(vec![
                Expression::Symbol("let".to_string()),
                Expression::List(bindings),
                if rewritten_body.len() == 1 {
                    rewritten_body[0].clone()
                } else {
                    let mut exprs = vec![Expression::Symbol("do".to_string())];
                    exprs.extend(rewritten_body.iter().cloned());
                    Expression::List(exprs)
                },
            ])]
        } else if needs_destructure {
            vec![Expression::List(vec![
                Expression::Symbol("let".to_string()),
                Expression::List(vec![Expression::List(vec![
                    item_pattern,
                    Expression::Symbol(item_name),
                ])]),
                if rewritten_body.len() == 1 {
                    rewritten_body[0].clone()
                } else {
                    let mut exprs = vec![Expression::Symbol("do".to_string())];
                    exprs.extend(rewritten_body.iter().cloned());
                    Expression::List(exprs)
                },
            ])]
        } else {
            rewritten_body
        };

        Ok(Expression::List(
            std::iter::once(Expression::Symbol("lambda".to_string()))
                .chain(std::iter::once(Expression::List(lambda_args)))
                .chain(rewritten_body)
                .collect(),
        ))
    }

    fn rewrite_each_bind_expr(
        &self,
        expr: &Expression,
        source: &Expression,
        item_name: &str,
        index_name: &str,
        zip_bind_sources: &HashMap<String, Expression>,
    ) -> Expression {
        let Expression::List(items) = expr else {
            return expr.clone();
        };

        if let Some(Expression::Symbol(name)) = items.first()
            && is_widget_name(name)
        {
            return self.rewrite_each_widget_bind(
                items,
                source,
                item_name,
                index_name,
                zip_bind_sources,
            );
        }

        Expression::List(
            items
                .iter()
                .map(|item| {
                    self.rewrite_each_bind_expr(
                        item,
                        source,
                        item_name,
                        index_name,
                        zip_bind_sources,
                    )
                })
                .collect(),
        )
    }

    fn rewrite_each_widget_bind(
        &self,
        items: &[Expression],
        source: &Expression,
        item_name: &str,
        index_name: &str,
        zip_bind_sources: &HashMap<String, Expression>,
    ) -> Expression {
        let mut out = Vec::with_capacity(items.len() + 4);
        let mut idx = 0;
        while idx < items.len() {
            match (items.get(idx), items.get(idx + 1)) {
                (Some(Expression::Keyword(key)), Some(Expression::Symbol(target)))
                    if key == "bind" && target == item_name =>
                {
                    out.push(Expression::Keyword("value".to_string()));
                    out.push(Expression::Symbol(target.clone()));
                    out.push(Expression::Keyword("on-change".to_string()));
                    out.push(Expression::List(vec![
                        Expression::Symbol("lambda".to_string()),
                        Expression::List(vec![Expression::Symbol("v".to_string())]),
                        Expression::List(vec![
                            Expression::Symbol("set!".to_string()),
                            source.clone(),
                            Expression::List(vec![
                                Expression::Symbol("set-nth".to_string()),
                                source.clone(),
                                Expression::Symbol(index_name.to_string()),
                                Expression::Symbol("v".to_string()),
                            ]),
                        ]),
                    ]));
                    idx += 2;
                }
                (Some(Expression::Keyword(key)), Some(Expression::Symbol(target)))
                    if key == "bind" && zip_bind_sources.contains_key(target) =>
                {
                    let zip_source = zip_bind_sources.get(target).expect("checked above").clone();
                    out.push(Expression::Keyword("value".to_string()));
                    out.push(Expression::Symbol(target.clone()));
                    out.push(Expression::Keyword("on-change".to_string()));
                    out.push(Expression::List(vec![
                        Expression::Symbol("lambda".to_string()),
                        Expression::List(vec![Expression::Symbol("v".to_string())]),
                        Expression::List(vec![
                            Expression::Symbol("set!".to_string()),
                            zip_source.clone(),
                            Expression::List(vec![
                                Expression::Symbol("set-nth".to_string()),
                                zip_source,
                                Expression::Symbol(index_name.to_string()),
                                Expression::Symbol("v".to_string()),
                            ]),
                        ]),
                    ]));
                    idx += 2;
                }
                (Some(item), _) => {
                    out.push(self.rewrite_each_bind_expr(
                        item,
                        source,
                        item_name,
                        index_name,
                        zip_bind_sources,
                    ));
                    idx += 1;
                }
                _ => break,
            }
        }
        Expression::List(out)
    }

    fn compile_widget_call(
        &mut self,
        widget_name: &str,
        list: &[Expression],
    ) -> Result<(), CompilerError> {
        let mut idx = 1;
        let mut arity = 0;
        while idx < list.len() {
            match list.get(idx) {
                Some(Expression::Keyword(key)) if key == "bind" => {
                    let Some(Expression::Symbol(target)) = list.get(idx + 1) else {
                        return Err(CompilerError::InvalidArg);
                    };

                    let value_kw = self.use_string_constant("value");
                    self.emit(OpCode::PushKeyword(value_kw));
                    arity += 1;
                    self.compile_expression(&Expression::Symbol(target.clone()))?;
                    arity += 1;

                    let on_change_kw = self.use_string_constant("on-change");
                    self.emit(OpCode::PushKeyword(on_change_kw));
                    arity += 1;
                    let callback_arg = "__eseq_bound_value".to_string();
                    let setter = Expression::List(vec![
                        Expression::Symbol("lambda".to_string()),
                        Expression::List(vec![Expression::Symbol(callback_arg.clone())]),
                        Expression::List(vec![
                            Expression::Symbol("set!".to_string()),
                            Expression::Symbol(target.clone()),
                            Expression::Symbol(callback_arg),
                        ]),
                    ]);
                    self.compile_expression(&setter)?;
                    arity += 1;
                    idx += 2;
                }
                // Auto-quote :material and :shader values (SDF shader expressions)
                Some(Expression::Keyword(key)) if key == "material" || key == "shader" => {
                    let kw_idx = self.use_string_constant(key);
                    self.emit(OpCode::PushKeyword(kw_idx));
                    arity += 1;
                    if let Some(val) = list.get(idx + 1) {
                        self.compile_quoted_expression(val)?;
                        arity += 1;
                    }
                    idx += 2;
                }
                Some(expr) => {
                    self.compile_expression(expr)?;
                    arity += 1;
                    idx += 1;
                }
                None => break,
            }
        }

        self.compile_expression(&Expression::Symbol(widget_name.to_string()))?;
        self.emit(OpCode::Call(arity));
        Ok(())
    }

    fn compile_inline_derived(&mut self, body: &[Expression]) -> Result<(), CompilerError> {
        let node_id = self.alloc_node_id();
        let chunk_idx = self.compile_reactive_chunk(node_id, body, false)?;
        self.emit(OpCode::InitDerived(node_id, chunk_idx));
        self.emit(OpCode::LoadDerived(node_id));
        Ok(())
    }

    fn compile_effect_form(&mut self, body: &[Expression]) -> Result<(), CompilerError> {
        let node_id = self.alloc_node_id();
        let chunk_idx = self.compile_reactive_chunk(node_id, body, true)?;
        self.emit(OpCode::InitEffect(node_id, chunk_idx));
        Ok(())
    }

    fn compile_effect_buffer_form(&mut self, body: &[Expression]) -> Result<(), CompilerError> {
        if body.len() < 2 {
            return Err(CompilerError::InvalidArg);
        }
        let target = match &body[0] {
            Expression::String(name) | Expression::Symbol(name) => name,
            _ => return Err(CompilerError::InvalidArg),
        };
        let node_id = self.alloc_node_id();
        let chunk_idx = self.compile_reactive_chunk(node_id, &body[1..], true)?;
        let target_idx = self.use_string_constant(target);
        self.emit(OpCode::InitNamedEffect(node_id, chunk_idx, target_idx));
        Ok(())
    }

    fn compile_subtree_form(&mut self, body: &[Expression]) -> Result<(), CompilerError> {
        if body.len() != 3 {
            return Err(CompilerError::InvalidArg);
        }
        let Expression::Keyword(key_kw) = &body[0] else {
            return Err(CompilerError::InvalidArg);
        };
        if key_kw != "key" {
            return Err(CompilerError::InvalidArg);
        }
        self.compile_expression(&body[1])?;
        let subtree_lambda = Expression::List(vec![
            Expression::Symbol("lambda".to_string()),
            Expression::List(vec![]),
            body[2].clone(),
        ]);
        self.compile_expression(&subtree_lambda)?;
        self.emit_symbol_load("subtree-owner");
        self.emit(OpCode::Call(2));
        Ok(())
    }

    fn chunk(&self) -> Option<&Chunk> {
        self.chunks.get(self.current_chunk)
    }

    fn chunk_mut(&mut self) -> Option<&mut Chunk> {
        self.chunks.get_mut(self.current_chunk)
    }

    fn use_constant(&mut self, num: f64) -> usize {
        let chunk = self.chunk_mut().unwrap();
        if let Some(index) = chunk.constants.iter().position(|r| *r == num) {
            return index;
        }
        let idx = chunk.constants.len();
        chunk.constants.push(num);
        idx
    }

    fn use_string_constant(&mut self, s: &str) -> usize {
        let chunk = self.chunk_mut().unwrap();
        if let Some(index) = chunk.strings.iter().position(|r| r == s) {
            return index;
        }
        let idx = chunk.strings.len();
        chunk.strings.push(s.to_string());
        idx
    }

    fn get_scope_mut(&mut self) -> &mut Scope {
        self.scopes.last_mut().unwrap()
    }

    /// True when `name` resolves to a local or upvalue somewhere in the scope
    /// chain (i.e. a builtin like `nth` has been shadowed). Read-only variant
    /// of resolve_symbol that never captures upvalues.
    fn symbol_is_locally_bound(&self, name: &str) -> bool {
        self.scopes.iter().any(|scope| {
            scope.symbols.iter().any(|s| s == name) || scope.upvalues.iter().any(|s| s == name)
        })
    }

    fn resolve_symbol(&mut self, name: &str) -> SymbolResolution {
        if let Some(idx) = self.get_scope_mut().symbols.iter().position(|s| *s == name) {
            return SymbolResolution::Local(idx);
        }

        // Walk the entire scope chain (from innermost parent outward) looking
        // for a local or upvalue that matches. This handles multi-level upvalue
        // capture needed by sequential let desugaring.
        let found_in_ancestor = self
            .scopes
            .iter()
            .rev()
            .skip(1) // skip current scope (already checked above)
            .any(|scope| {
                scope.symbols.iter().any(|s| *s == name)
                    || scope.upvalues.iter().any(|s| *s == name)
            });

        if found_in_ancestor {
            let current = self.get_scope_mut();
            let upvalues_idx = current.upvalues.len();
            current.upvalues.push(name.to_string());
            return SymbolResolution::Upvalue(upvalues_idx);
        }

        let idx = self.use_global(name);
        SymbolResolution::Global(idx)
    }

    fn use_global(&mut self, name: &str) -> usize {
        let resolved = self.resolve_global_name(name);
        if let Some(index) = self.global_symbols.iter().position(|r| *r == resolved) {
            return index;
        }
        let idx = self.global_symbols.len();
        self.global_symbols.push(resolved);
        idx
    }

    /// Resolution ladder for global names (module-system spec §3).
    /// Qualified `X/name`: `X` resolves as an import alias first, then as a
    /// namespace (current module, blessed core, implicit, or one with
    /// existing definitions); unknown alias/namespace is a compile error.
    /// Bare names: current module → `:refer`red symbols → flat entry (Rust
    /// natives and host-registered globals stay unqualified) → intern new
    /// names under the current module. Mirrored at runtime by
    /// `VM::resolve_global_read_index` — keep the two in sync.
    fn resolve_global_name(&mut self, name: &str) -> String {
        if self.reactive_namespaces.contains(name) {
            return name.to_string();
        }
        if let Some((ns, base)) = super::modules::split_qualified(name) {
            let full_ns = self
                .import_aliases
                .get(ns)
                .cloned()
                .unwrap_or_else(|| ns.to_string());
            if full_ns != self.current_module && self.hidden_by(&full_ns, base) {
                self.warn_once(format!(
                    "warning: {full_ns}/{base} is not exported by {full_ns}; \
                     referencing it from {} may break on update",
                    self.current_module
                ));
            }
            if !self.namespace_is_known(&full_ns) {
                if full_ns.contains('.') {
                    // Dotted = full module name: the §3 escape hatch may
                    // define into or reference a module before it loads, so
                    // this warns instead of erroring and the namespace
                    // becomes known for the rest of the unit.
                    self.warn_once(format!(
                        "warning: namespace '{full_ns}' in '{name}' has no import and no \
                         definitions yet (cross-module escape hatch, or a load-order gap)"
                    ));
                    self.known_namespaces.insert(full_ns.clone());
                } else {
                    // Undotted = alias-shaped: a typo'd or missing import.
                    self.errors.push(format!(
                        "unknown alias or namespace '{ns}' in reference '{name}' \
                         (no import in scope and no module of that name)"
                    ));
                }
            }
            let resolved = super::modules::qualify(&full_ns, base);
            // Core namespaces resolve without import, and their natives
            // may still be registered flat: `eseq.core/label` (or an
            // explicit `eseq.vanilla/x`) falls back to the flat entry when
            // no qualified one exists (mirrors VM::resolve_global_read_index).
            if (super::modules::CORE_NAMESPACES.contains(&full_ns.as_str())
                || full_ns == super::modules::IMPLICIT_MODULE)
                && !self.global_symbols.iter().any(|s| *s == resolved)
                && self.global_symbols.iter().any(|s| s == base)
            {
                return base.to_string();
            }
            return resolved;
        }
        let qualified = super::modules::qualify(&self.current_module, name);
        // A declared module's own entry wins over everything but lexical
        // scope (spec §3); the implicit eseq.vanilla entry is checked
        // AFTER compat aliases below, because a converted symbol's stale
        // vanilla slot (left interned from before the conversion) is
        // exactly what an alias exists to supersede.
        if self.declared_module().is_some() && self.global_symbols.iter().any(|s| *s == qualified)
        {
            return qualified;
        }
        if let Some(target) = self.refers.get(name) {
            return target.clone();
        }
        if self.global_symbols.iter().any(|s| *s == qualified) {
            return qualified;
        }
        if self.global_symbols.iter().any(|s| *s == name) {
            return name.to_string();
        }
        qualified
    }

    /// A namespace is known if it is the current module, the implicit
    /// module, a blessed core namespace, an imported module, one a
    /// qualified `def` in this unit targets, or one with existing
    /// definitions in the global or macro tables (covers hand-rolled
    /// legacy names and modules loaded earlier).
    fn namespace_is_known(&self, ns: &str) -> bool {
        if ns == self.current_module
            || ns == super::modules::IMPLICIT_MODULE
            || super::modules::CORE_NAMESPACES.contains(&ns)
            || self.known_namespaces.contains(ns)
            || self.import_aliases.values().any(|full| full == ns)
        {
            return true;
        }
        let prefix_len = ns.len();
        let has_prefix = |key: &str| {
            key.len() > prefix_len + 1
                && key.as_bytes()[prefix_len] == b'/'
                && key.starts_with(ns)
        };
        self.global_symbols.iter().any(|s| has_prefix(s))
            || self.macros.keys().any(|k| has_prefix(k))
    }

    /// The declared (non-implicit) module of this compile unit, if any.
    fn declared_module(&self) -> Option<String> {
        (self.current_module != super::modules::IMPLICIT_MODULE)
            .then(|| self.current_module.clone())
    }

    /// Registry auto-qualification (spec §5): registration-form names
    /// (`defstate`, `def-process`, …) prefix the current module unless
    /// already qualified. Headerless (eseq.vanilla) files keep flat keys —
    /// that is what keeps serialized identity stable until a file converts.
    ///
    /// An explicit `eseq.vanilla/name` registration (the §3 cross-module def
    /// escape hatch, used by hazard (i) to pin a `defstate` that Rust writes
    /// by bare spelling) registers under the **flat** key: vanilla's registry
    /// keyspace *is* the flat keyspace under slice 0, and neither
    /// `Compiler::state_binding_for` nor `VM::state_binding_node` has an
    /// implicit-module rung. Without this strip a pinned `(defstate
    /// eseq.vanilla/sbrowser-tab …)` would key `state_bindings` as
    /// `eseq.vanilla/sbrowser-tab` while every flat reader and writer looks up
    /// `sbrowser-tab`, so the binding would be invisible and `(set! …)` would
    /// StoreGlobal over the slot holding the NodeRef.
    fn qualify_registration_name(&self, name: &str) -> String {
        if super::modules::is_qualified(name) {
            return super::modules::strip_implicit(name).to_string();
        }
        if self.declared_module().is_none() {
            return name.to_string();
        }
        super::modules::qualify(&self.current_module, name)
    }

    /// State-binding lookup mirroring the §3 resolution ladder over the
    /// (possibly qualified) `state_bindings` keyspace: exact key →
    /// current-module key → `:refer` target.
    fn state_binding_for(&self, name: &str) -> Option<u32> {
        if let Some(id) = self.state_bindings.get(name) {
            return Some(*id);
        }
        if let Some((ns, base)) = super::modules::split_qualified(name) {
            let full_ns = self
                .import_aliases
                .get(ns)
                .map(String::as_str)
                .unwrap_or(ns);
            // `eseq.vanilla/x` registers flat (see `qualify_registration_name`),
            // so the explicit escape-hatch spelling has to reduce to the flat
            // key or a pinned `defstate` read qualified would compile to
            // LoadGlobal and hand back the raw NodeRef.
            if full_ns == super::modules::IMPLICIT_MODULE {
                return self.state_bindings.get(base).copied();
            }
            return self
                .state_bindings
                .get(&super::modules::qualify(full_ns, base))
                .copied();
        }
        if self.declared_module().is_some()
            && let Some(id) = self
                .state_bindings
                .get(&super::modules::qualify(&self.current_module, name))
        {
            return Some(*id);
        }
        if let Some(id) = self
            .refers
            .get(name)
            .and_then(|qualified| self.state_bindings.get(qualified))
        {
            return Some(*id);
        }
        None
    }

    fn warn_once(&mut self, message: String) {
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    pub fn new_chunk(&mut self, mut chunk: Chunk) -> (usize, usize) {
        // Stamp every chunk with the module current at its creation
        // (None = implicit eseq.vanilla). The entry chunk of a unit whose
        // (module …) form appears mid-file is re-stamped by that form.
        chunk.source_module = self.declared_module();
        let symbols = chunk.symbols.clone();
        let prev_chunk_idx = self.current_chunk;
        let new_chunk_idx = self.chunks.len();
        self.chunks.push(chunk);
        self.current_chunk = new_chunk_idx;

        self.scopes.push(Scope {
            chunk_idx: new_chunk_idx,
            symbols,
            upvalues: vec![],
        });

        (new_chunk_idx, prev_chunk_idx)
    }

    fn emit(&mut self, op: OpCode) {
        self.chunk_mut().unwrap().ops.push(op);
    }

    fn emit_symbol_load(&mut self, name: &str) {
        match self.resolve_symbol(name) {
            SymbolResolution::Global(idx) => match self.state_binding_for(name) {
                Some(node_id) => self.emit(OpCode::LoadState(node_id)),
                None => self.emit(OpCode::LoadGlobal(idx)),
            },
            SymbolResolution::Local(idx) => self.emit(OpCode::LoadLocal(idx)),
            SymbolResolution::Upvalue(idx) => self.emit(OpCode::LoadUpvalue(idx)),
        }
    }

    fn emit_symbol_store(&mut self, name: &str) {
        match self.resolve_symbol(name) {
            SymbolResolution::Global(idx) => match self.state_binding_for(name) {
                Some(node_id) => self.emit(OpCode::StoreState(node_id)),
                None => self.emit(OpCode::StoreGlobal(idx)),
            },
            SymbolResolution::Local(idx) => self.emit(OpCode::StoreLocal(idx)),
            SymbolResolution::Upvalue(idx) => self.emit(OpCode::StoreUpvalue(idx)),
        }
    }

    /// Definition sites (`def`, defstate, derived defs) in a DECLARED
    /// module always intern under that module: the reference ladder's
    /// flat-entry fallback exists so vanilla defs keep today's
    /// shadow-natives-in-place semantics, but inside a module it would
    /// silently define into (and clobber) a same-named flat native — the
    /// `select` builtin-widget collision found in S3 batch 1. Shadowing a
    /// flat/core name from a module warns (spec §3) and creates the
    /// module's own entry instead.
    fn emit_symbol_store_for_definition(&mut self, name: &str) {
        if self.declared_module().is_some()
            && !super::modules::is_qualified(name)
            && !self.reactive_namespaces.contains(name)
        {
            let idx = self.use_global_for_definition(name);
            match self.state_binding_for(name) {
                Some(node_id) => self.emit(OpCode::StoreState(node_id)),
                None => self.emit(OpCode::StoreGlobal(idx)),
            }
            return;
        }
        self.emit_symbol_store(name);
    }

    /// Index for a definition-site global: in a declared module, always the
    /// module-qualified entry (see `emit_symbol_store_for_definition`);
    /// otherwise the ordinary reference ladder.
    fn use_global_for_definition(&mut self, name: &str) -> usize {
        if self.declared_module().is_none()
            || super::modules::is_qualified(name)
            || self.reactive_namespaces.contains(name)
        {
            return self.use_global(name);
        }
        if self.global_symbols.iter().any(|s| s == name) {
            let module = self.current_module.clone();
            self.warn_once(format!(
                "warning: (def {name} …) in {module} does not replace the \
                 existing flat/core `{name}`; it defines {module}/{name} \
                 (use an explicit qualified def to overwrite the flat entry)"
            ));
        }
        let qualified = super::modules::qualify(&self.current_module, name);
        if let Some(index) = self.global_symbols.iter().position(|r| *r == qualified) {
            return index;
        }
        let idx = self.global_symbols.len();
        self.global_symbols.push(qualified);
        idx
    }

    pub fn compile_function(
        &mut self,
        name: Option<String>,
        args: Vec<Expression>,
        body: Vec<Expression>,
    ) -> Result<(), CompilerError> {
        let mut symbols = Vec::with_capacity(args.len());
        let mut wrapped_body = if body.len() == 1 {
            body[0].clone()
        } else {
            let mut exprs = vec![Expression::Symbol("do".to_string())];
            exprs.extend(body.iter().cloned());
            Expression::List(exprs)
        };
        let mut arg_symbols = Vec::with_capacity(args.len());
        for arg in args.iter() {
            match arg {
                Expression::Symbol(s) => arg_symbols.push(s.to_string()),
                Expression::List(_) => {
                    let temp = self.alloc_temp_symbol("arg");
                    arg_symbols.push(temp);
                }
                _ => return Err(CompilerError::InvalidArg),
            }
        }
        for (arg, symbol) in args.iter().zip(arg_symbols.iter()).rev() {
            if matches!(arg, Expression::List(_)) {
                wrapped_body = self.desugar_pattern_binding(
                    arg,
                    Expression::Symbol(symbol.clone()),
                    wrapped_body,
                )?;
            }
        }
        symbols.extend(arg_symbols);
        let (new_chunk_idx, previous_chunk_idx) = self.new_chunk(Chunk {
            ops: vec![],
            constants: vec![],
            strings: vec![],
            symbols,
            upvalues: vec![],
            source_symbol: name.clone(),
            source_file: self.source_file.clone(),
            source_module: None,
        });
        self.compile_expression(&wrapped_body)?;

        let scope = self.scopes.pop().unwrap();
        if let Some(chunk) = self.chunks.get_mut(new_chunk_idx) {
            chunk.upvalues = scope.upvalues.clone();
        }
        self.emit(OpCode::Return);
        self.current_chunk = previous_chunk_idx;

        for upvalue_name in &scope.upvalues {
            let resolved = self.resolve_symbol(upvalue_name);
            match resolved {
                SymbolResolution::Local(idx) => self.emit(OpCode::LoadLocal(idx)),
                SymbolResolution::Upvalue(idx) => self.emit(OpCode::LoadUpvalue(idx)),
                _ => {}
            }
        }
        self.emit(OpCode::MakeClosure(new_chunk_idx, scope.upvalues.len()));
        if let Some(name) = name {
            self.emit_symbol_store_for_definition(&name);
            self.emit(OpCode::PushNil);
        }

        Ok(())
    }

    pub fn op_idx(&self) -> usize {
        self.chunk().unwrap().ops.len()
    }

    pub fn compile_if_statement(
        &mut self,
        condition: Expression,
        then_body: Expression,
        else_body: Expression,
    ) -> Result<(), CompilerError> {
        self.compile_expression(&condition)?;
        let jump_op_idx = self.op_idx();
        self.emit(OpCode::JumpIfFalse(0));
        self.compile_expression(&then_body)?;
        let then_end_idx = self.op_idx();
        self.emit(OpCode::Jump(0));
        let else_begin_idx = self.op_idx();
        let jump_false_increment = else_begin_idx - jump_op_idx;
        self.chunk_mut().unwrap().ops[jump_op_idx] = OpCode::JumpIfFalse(jump_false_increment);
        self.compile_expression(&else_body)?;
        let else_end_idx = self.op_idx();
        self.chunk_mut().unwrap().ops[then_end_idx] = OpCode::Jump(else_end_idx - then_end_idx);
        Ok(())
    }

    pub fn compile_match_statement(
        &mut self,
        value: Expression,
        branches: Vec<(Expression, Expression)>,
        default: Option<Expression>,
    ) -> Result<(), CompilerError> {
        let match_value_symbol = Expression::Symbol("__eseq_match_value".to_string());
        let default_expr = default.unwrap_or(Expression::Symbol("nil".to_string()));
        let nested_ifs =
            branches
                .into_iter()
                .rev()
                .fold(default_expr, |else_body, (pattern, body)| {
                    Expression::List(vec![
                        Expression::Symbol("if".to_string()),
                        Expression::List(vec![
                            Expression::Symbol("=".to_string()),
                            match_value_symbol.clone(),
                            pattern,
                        ]),
                        body,
                        else_body,
                    ])
                });
        let desugared = Expression::List(vec![
            Expression::Symbol("let".to_string()),
            Expression::List(vec![Expression::List(vec![match_value_symbol, value])]),
            nested_ifs,
        ]);
        self.compile_expression(&desugared)
    }

    fn compile_block(&mut self, expressions: &[Expression]) -> Result<(), CompilerError> {
        if expressions.is_empty() {
            self.emit(OpCode::PushNil);
            return Ok(());
        }

        for (idx, expression) in expressions.iter().enumerate() {
            self.compile_expression(expression)?;
            if idx + 1 < expressions.len() {
                self.emit(OpCode::Pop);
            }
        }
        Ok(())
    }

    fn compile_let_statement(
        &mut self,
        bindings_expr: &Expression,
        body: &[Expression],
    ) -> Result<(), CompilerError> {
        let Expression::List(bindings) = bindings_expr else {
            return Err(CompilerError::InvalidArg);
        };

        // Sequential binding (let*): each binding can reference previous ones.
        // Desugared into nested single-binding lets:
        //   (let ((a 1) (b a)) body)
        //   → ((fn (a) ((fn (b) body) a)) 1)
        let desugared = self.desugar_let_sequential(bindings, body)?;
        self.compile_expression(&desugared)
    }

    fn desugar_let_sequential(
        &mut self,
        bindings: &[Expression],
        body: &[Expression],
    ) -> Result<Expression, CompilerError> {
        if bindings.is_empty() {
            if body.len() == 1 {
                return Ok(body[0].clone());
            }
            let mut exprs = vec![Expression::Symbol("do".to_string())];
            exprs.extend_from_slice(body);
            return Ok(Expression::List(exprs));
        }

        let Expression::List(pair) = &bindings[0] else {
            return Err(CompilerError::InvalidArg);
        };
        if pair.len() != 2 {
            return Err(CompilerError::InvalidArg);
        }

        let inner = self.desugar_let_sequential(&bindings[1..], body)?;
        self.desugar_pattern_binding(&pair[0], pair[1].clone(), inner)
    }

    pub fn compile_list(&mut self, list: &[Expression]) -> Result<(), CompilerError> {
        // Macro expansion: if the head is a macro name, expand and compile the result
        if let Some(Expression::Symbol(name)) = list.first() {
            if let Some(mac) = self.lookup_macro(name) {
                let arg_count = list.len() - 1;
                let arity_matches = match &mac.rest_param {
                    Some(_) => arg_count >= mac.params.len(),
                    None => arg_count == mac.params.len(),
                };
                if !arity_matches {
                    return Err(CompilerError::InvalidArg);
                }
                let call_expr = Expression::List(list.to_vec());
                let expanded = self.expand_macros(&call_expr, 0);
                if expanded == call_expr {
                    return Err(CompilerError::InvalidArg);
                }
                return self.compile_expression(&expanded);
            }
        }

        // Indexed reactive reads — (nth SEQ.field idx) and (len SEQ.field) —
        // compile to dedicated opcodes so the reactive DAG can record
        // per-index dependencies and skip subtree reruns when untouched
        // elements of a list change.
        if let Some(Expression::Symbol(head)) = list.first()
            && ((head == "nth" && list.len() == 3) || (head == "len" && list.len() == 2))
            && let Some(Expression::Symbol(target)) = list.get(1)
            && !self.symbol_is_locally_bound(head)
            && !self.derived_bindings.contains_key(target)
        {
            let parts: Vec<&str> = target.splitn(2, '.').collect();
            if parts.len() == 2
                && !parts[0].is_empty()
                && !parts[1].is_empty()
                && !parts[1].contains('.')
                && self.reactive_namespaces.contains(parts[0])
            {
                let ns_idx = self.use_string_constant(parts[0]);
                let field_idx = self.use_string_constant(parts[1]);
                if head == "nth" {
                    self.compile_expression(&list[2])?;
                    self.emit(OpCode::LoadReactiveNth(ns_idx, field_idx));
                } else {
                    self.emit(OpCode::LoadReactiveLen(ns_idx, field_idx));
                }
                return Ok(());
            }
        }

        if let Some(Expression::Symbol(widget_name)) = list.first()
            && is_widget_name(widget_name)
            && list
                .iter()
                .any(|expr| matches!(expr, Expression::Keyword(key) if key == "bind"))
        {
            return self.compile_widget_call(widget_name, list);
        }

        if let [Expression::Symbol(each), source, lambda] = list
            && each == "each"
        {
            return self.compile_each_form(source, lambda);
        }

        if let [
            Expression::Symbol(def),
            Expression::Symbol(name),
            Expression::List(state),
        ] = list
            && def == "def"
            && let Some(Expression::Symbol(form)) = state.first()
            && form == "state"
            && state.len() == 2
        {
            return self.compile_named_state_definition(name, &state[1]);
        }

        if let [
            Expression::Symbol(def),
            Expression::Symbol(name),
            Expression::List(derived),
        ] = list
            && def == "def"
            && let Some(Expression::Symbol(form)) = derived.first()
            && form == "derived"
        {
            return self.compile_named_derived_definition(name, &derived[1..]);
        }

        if let Some((name, args, body)) = extract_function_definition(list) {
            return self.compile_function(name, args, body);
        }

        if let Some((cond, then_body, else_body)) = extract_if_statement(list) {
            return self.compile_if_statement(cond, then_body, else_body);
        }

        if let Some((value, branches, default)) = extract_match_statement(list) {
            return self.compile_match_statement(value, branches, default);
        }

        if let Some(exprs) = extract_boolean_chain(list, "and") {
            let desugared = self.desugar_and(exprs)?;
            return self.compile_expression(&desugared);
        }

        if let Some(exprs) = extract_boolean_chain(list, "or") {
            let desugared = self.desugar_or(exprs)?;
            return self.compile_expression(&desugared);
        }

        if let [Expression::Symbol(thread), initial, stages @ ..] = list
            && (thread == "->" || thread == "->>")
        {
            let desugared = self.desugar_threading(initial, stages, thread == "->>")?;
            return self.compile_expression(&desugared);
        }

        // (eval expr) — compile expr to produce a string, then evaluate it at runtime
        if let (Some(Expression::Symbol(s)), Some(expr), 2) =
            (list.first(), list.get(1), list.len())
            && s == "eval"
        {
            self.compile_expression(expr)?;
            self.emit(OpCode::Eval);
            return Ok(());
        }

        if let Some(Expression::Symbol(s)) = list.first() {
            // (module NAME) — top-level declaration (spec §2 decision 1):
            // everything after it in this compile unit belongs to NAME.
            if s == "module" {
                if self.scopes.len() > 1 {
                    self.errors
                        .push("(module …) must appear at top level".to_string());
                } else if let (Some(Expression::Symbol(name)), 2) = (list.get(1), list.len()) {
                    if !super::modules::is_valid_module_name(name) {
                        self.errors.push(format!(
                            "invalid module name '{name}' (dotted segments, no '/')"
                        ));
                    } else if self.module_declared {
                        self.errors.push(format!(
                            "duplicate (module {name}) — one module form per file"
                        ));
                    } else {
                        self.module_declared = true;
                        self.current_module = name.clone();
                        // The unit's entry chunk was created before this
                        // form was seen; everything after it in the file
                        // belongs to the module, so re-stamp it (top-level
                        // registration calls run in the entry chunk).
                        let entry_idx = self.current_chunk;
                        if let Some(chunk) = self.chunks.get_mut(entry_idx) {
                            chunk.source_module = Some(name.clone());
                        }
                        // Register the declaration at runtime so `import`
                        // can treat the module as loaded (load-once).
                        let declare = Expression::List(vec![
                            Expression::Symbol("__module-declare".to_string()),
                            Expression::String(name.clone()),
                        ]);
                        return self.compile_expression(&declare);
                    }
                } else {
                    self.errors
                        .push("(module …) expects exactly one symbol name".to_string());
                }
                self.emit(OpCode::PushNil);
                return Ok(());
            }
            // (export sym …) — top-level, named-module-only declaration.
            // Runtime registration makes the same form append at the REPL;
            // file loads replace their set up front in eval_module_source.
            if s == "export" {
                if self.scopes.len() > 1 {
                    self.errors.push(
                        "(export …) must appear at top level, not inside a function".to_string(),
                    );
                    self.emit(OpCode::PushNil);
                    return Ok(());
                }
                if !self.module_declared {
                    self.errors.push(
                        "(export …) requires a named module; declare a module first".to_string(),
                    );
                    self.emit(OpCode::PushNil);
                    return Ok(());
                }
                let mut register = vec![
                    Expression::Symbol("__module-export".to_string()),
                    Expression::String(self.current_module.clone()),
                ];
                for entry in &list[1..] {
                    let Expression::Symbol(name) = entry else {
                        let message = if matches!(entry, Expression::List(_)) {
                            "(export …) list-shaped entries are reserved for re-export"
                        } else {
                            "(export …) expects bare symbol names"
                        };
                        self.errors.push(message.to_string());
                        continue;
                    };
                    if name.contains('/') {
                        self.errors.push(format!(
                            "export name '{name}' must be a bare, unqualified symbol"
                        ));
                        continue;
                    }
                    register.push(Expression::String(name.clone()));
                }
                return self.compile_expression(&Expression::List(register));
            }
            // (import NAME :as ALIAS) / (import NAME :refer (sym …)) —
            // load-once + alias/refer registration (spec §2 decision 2, §4).
            if s == "import" {
                if self.scopes.len() > 1 {
                    self.errors
                        .push("(import …) must appear at top level, not inside a function".to_string());
                    self.emit(OpCode::PushNil);
                    return Ok(());
                }
                let Some(Expression::Symbol(module_name)) = list.get(1) else {
                    self.errors
                        .push("(import …) expects a module name symbol".to_string());
                    self.emit(OpCode::PushNil);
                    return Ok(());
                };
                if !super::modules::is_valid_module_name(module_name) {
                    self.errors
                        .push(format!("invalid module name '{module_name}' in import"));
                    self.emit(OpCode::PushNil);
                    return Ok(());
                }
                let mut i = 2;
                while i < list.len() {
                    match (&list[i], list.get(i + 1)) {
                        (Expression::Keyword(k), Some(Expression::Symbol(alias)))
                            if k == "as" =>
                        {
                            self.import_aliases
                                .insert(alias.clone(), module_name.clone());
                            i += 2;
                        }
                        (Expression::Keyword(k), Some(Expression::List(syms)))
                            if k == "refer" =>
                        {
                            for sym in syms {
                                let Expression::Symbol(sym) = sym else {
                                    self.errors.push(format!(
                                        "import {module_name}: :refer expects symbols"
                                    ));
                                    continue;
                                };
                                self.refers.insert(
                                    sym.clone(),
                                    super::modules::qualify(module_name, sym),
                                );
                            }
                            i += 2;
                        }
                        _ => {
                            self.errors.push(format!(
                                "import {module_name}: expected :as ALIAS or :refer (sym …)"
                            ));
                            break;
                        }
                    }
                }
                self.known_namespaces.insert(module_name.clone());
                // Runtime half: resolve the name to a file and evaluate it
                // if and only if it has not been evaluated (spec §4).
                let load = Expression::List(vec![
                    Expression::Symbol("__import-module".to_string()),
                    Expression::String(module_name.clone()),
                ]);
                return self.compile_expression(&load);
            }
            // Advice-style user overrides (spec §6.1). The target is syntax,
            // not a value lookup: compile it to a canonical qualified string
            // plus a callback. Supported spellings are:
            //   (override module/name (lambda (args...) ...))
            //   (override module/name (args...) body...)
            //   (override module/name :around (original args...) body...)
            if s == "override" {
                let Some(Expression::Symbol(target)) = list.get(1) else {
                    self.errors.push(
                        "(override …) expects a qualified symbol target".to_string(),
                    );
                    self.emit(OpCode::PushNil);
                    return Ok(());
                };
                let Some((namespace, base)) = super::modules::split_qualified(target) else {
                    self.errors.push(format!(
                        "override target '{target}' must be module-qualified"
                    ));
                    self.emit(OpCode::PushNil);
                    return Ok(());
                };
                let namespace = self
                    .import_aliases
                    .get(namespace)
                    .cloned()
                    .unwrap_or_else(|| namespace.to_string());
                let target = super::modules::qualify(&namespace, base);
                if self.hidden_by(&namespace, base) {
                    self.warn_once(format!(
                        "warning: overriding {target}, which is not exported by {namespace}; \
                         this override may break on update"
                    ));
                }

                let (kind, callback) = match list.get(2) {
                    Some(Expression::Keyword(kind)) if kind == "around" => {
                        let Some(Expression::List(args)) = list.get(3) else {
                            self.errors.push(
                                "(override … :around …) expects (original args...) and a body"
                                    .to_string(),
                            );
                            self.emit(OpCode::PushNil);
                            return Ok(());
                        };
                        if args.is_empty() || list.len() < 5 {
                            self.errors.push(
                                "(override … :around …) requires original and a body".to_string(),
                            );
                            self.emit(OpCode::PushNil);
                            return Ok(());
                        }
                        let mut lambda = vec![
                            Expression::Symbol("lambda".to_string()),
                            Expression::List(args.clone()),
                        ];
                        lambda.extend_from_slice(&list[4..]);
                        ("around", Expression::List(lambda))
                    }
                    Some(Expression::List(items))
                        if matches!(items.first(), Some(Expression::Symbol(head)) if head == "lambda")
                            && list.len() == 3 =>
                    {
                        ("replace", list[2].clone())
                    }
                    Some(Expression::List(args)) if list.len() >= 4 => {
                        let mut lambda = vec![
                            Expression::Symbol("lambda".to_string()),
                            Expression::List(args.clone()),
                        ];
                        lambda.extend_from_slice(&list[3..]);
                        ("replace", Expression::List(lambda))
                    }
                    Some(callback) if list.len() == 3 => ("replace", callback.clone()),
                    _ => {
                        self.errors.push(
                            "(override …) expects a callback or argument list and body".to_string(),
                        );
                        self.emit(OpCode::PushNil);
                        return Ok(());
                    }
                };
                let register = Expression::List(vec![
                    Expression::Symbol("__register-override".to_string()),
                    Expression::String(target),
                    Expression::String(kind.to_string()),
                    callback,
                ]);
                return self.compile_expression(&register);
            }
            if s == "remove-override" {
                let Some(Expression::Symbol(target)) = list.get(1) else {
                    self.errors.push(
                        "(remove-override …) expects a qualified symbol target".to_string(),
                    );
                    self.emit(OpCode::PushNil);
                    return Ok(());
                };
                if list.len() != 2 {
                    self.errors.push(
                        "(remove-override …) expects exactly one target".to_string(),
                    );
                    self.emit(OpCode::PushNil);
                    return Ok(());
                }
                let Some((namespace, base)) = super::modules::split_qualified(target) else {
                    self.errors.push(format!(
                        "remove-override target '{target}' must be module-qualified"
                    ));
                    self.emit(OpCode::PushNil);
                    return Ok(());
                };
                let namespace = self
                    .import_aliases
                    .get(namespace)
                    .map(String::as_str)
                    .unwrap_or(namespace);
                let remove = Expression::List(vec![
                    Expression::Symbol("__remove-override".to_string()),
                    Expression::String(super::modules::qualify(namespace, base)),
                ]);
                return self.compile_expression(&remove);
            }
            if s == "defmacro" && list.len() == 4 {
                let Expression::Symbol(name) = &list[1] else {
                    return Err(CompilerError::InvalidArg);
                };
                let Expression::List(params_expr) = &list[2] else {
                    return Err(CompilerError::InvalidArg);
                };
                let mut params = Vec::new();
                let mut param_names = HashSet::new();
                let mut rest_param = None;
                let mut index = 0;
                while index < params_expr.len() {
                    let Expression::Symbol(param) = &params_expr[index] else {
                        return Err(CompilerError::InvalidArg);
                    };
                    if param == "&rest" {
                        let Some(Expression::Symbol(rest)) = params_expr.get(index + 1) else {
                            return Err(CompilerError::InvalidArg);
                        };
                        if index + 2 != params_expr.len()
                            || rest == "&rest"
                            || !param_names.insert(rest.clone())
                        {
                            return Err(CompilerError::InvalidArg);
                        }
                        rest_param = Some(rest.clone());
                        break;
                    }
                    if !param_names.insert(param.clone()) {
                        return Err(CompilerError::InvalidArg);
                    }
                    params.push(param.clone());
                    index += 1;
                }
                // Inside a declared module, bare macro names intern
                // qualified (`sdf/circle`); headerless (eseq.vanilla)
                // files keep flat keys until slice 3 so the patcher's
                // textual defmacro machinery is untouched.
                let key = if !super::modules::is_qualified(name)
                    && self.current_module != super::modules::IMPLICIT_MODULE
                {
                    super::modules::qualify(&self.current_module, name)
                } else {
                    name.clone()
                };
                self.macros.insert(
                    key,
                    MacroDef {
                        params,
                        rest_param,
                        body: list[3].clone(),
                    },
                );
                self.emit(OpCode::PushNil);
                return Ok(());
            }
            if s == "defwidget" && list.len() >= 4 {
                // (defwidget name :key val ... :shader (sdf/layer ...))
                // Compile as a call to the "defwidget" native, but:
                // 1. Convert the name symbol to a string
                // 2. Auto-quote the :shader value so it's not evaluated
                let Expression::Symbol(widget_name) = &list[1] else {
                    return Err(CompilerError::InvalidArg);
                };
                let name_str = self.use_string_constant(widget_name);
                self.emit(OpCode::PushStr(name_str));
                let mut i = 2;
                let mut arity = 1; // name is first arg
                while i < list.len() {
                    if let Expression::Keyword(key) = &list[i] {
                        let idx = self.use_string_constant(key);
                        self.emit(OpCode::PushKeyword(idx));
                        arity += 1;
                        if (key == "shader" || key == "state" || key == "bindable")
                            && i + 1 < list.len()
                        {
                            // Auto-quote shader, state, and bindable expressions.
                            self.compile_quoted_expression(&list[i + 1])?;
                        } else if i + 1 < list.len() {
                            self.compile_expression(&list[i + 1])?;
                        }
                        arity += 1;
                        i += 2;
                    } else {
                        self.compile_expression(&list[i])?;
                        arity += 1;
                        i += 1;
                    }
                }
                self.emit_symbol_load("defwidget");
                self.emit(OpCode::Call(arity));
                return Ok(());
            }
            if s == "do" {
                return self.compile_block(&list[1..]);
            }
            if s == "set!" && list.len() == 3 {
                return self.compile_set_statement(&list[1], &list[2]);
            }
            if s == "defstate" && list.len() == 3 {
                let Expression::Symbol(name) = &list[1] else {
                    return Err(CompilerError::InvalidArg);
                };
                return self.compile_named_state_definition(name, &list[2]);
            }
            if s == "defcustom" {
                return self.compile_custom_definition(list);
            }
            if s == "setopt" && list.len() == 3 {
                return self.compile_set_statement(&list[1], &list[2]);
            }
            if s == "effect" {
                return self.compile_effect_form(&list[1..]);
            }
            if s == "effect-buffer" {
                return self.compile_effect_buffer_form(&list[1..]);
            }
            if s == "subtree" {
                return self.compile_subtree_form(&list[1..]);
            }
            if s == "derived" {
                return self.compile_inline_derived(&list[1..]);
            }
            if s == "state" {
                return Err(CompilerError::InvalidArg);
            }
            if s == "let" && list.len() >= 3 {
                return self.compile_let_statement(&list[1], &list[2..]);
            }
        }

        let op = list.first();

        if let Some(op) = op {
            // Auto-quote the value following :material or :shader keywords,
            // since these contain SDF shader expressions (with variables like
            // value_t, x, y) that must not be evaluated at runtime.
            //
            // `def-sequencer` does the same for :tick / :init: the body is a
            // program for the *sequencer* runtime, so it must be captured as data
            // here (in the UI/editor runtime) and shipped, not evaluated locally.
            let is_def_sequencer = matches!(op, Expression::Symbol(s) if s == "def-sequencer");
            let is_def_process = matches!(op, Expression::Symbol(s) if s == "def-process");
            let is_def_accumulator = matches!(op, Expression::Symbol(s) if s == "def-accumulator");
            let is_defchan = matches!(op, Expression::Symbol(s) if s == "defchan");
            let is_process_sugar = matches!(op, Expression::Symbol(s)
                if s == "every" || s == "after" || s == "on" || s == "tap");
            // `def-song` (song-mode declarative authoring): the whole body —
            // `(at <beat> :scene n :patterns ((track pat)...))` rows plus
            // :end/:loop — is a declaration for the host, not code, so it is
            // captured as data like def-sequencer's graph mode.
            let is_def_song = matches!(op, Expression::Symbol(s) if s == "def-song");
            // Graph-mode `def-sequencer`: the *entire* body (`:shape`, sequencer-level
            // config, `def-node` and `edges` sub-forms) is a manifest for the sequencer
            // runtime and must be captured as data, not evaluated here. Detected by the
            // presence of a `def-node` sub-form (its absence selects the existing tick
            // mode, whose path below is left untouched). `,x` (Unquote) at the top level
            // still escapes to runtime evaluation, so computed config composes — this is
            // the auto-quasiquote of the whole body.
            let is_graph_sequencer = is_def_sequencer
                && list.iter().skip(1).any(|elem| {
                    matches!(elem, Expression::List(l)
                        if matches!(l.first(), Some(Expression::Symbol(s)) if s == "def-node"))
                });
            let mut quote_next = false;
            let mut quote_next_preserving = false;
            for (i, elem) in list.iter().skip(1).enumerate() {
                if is_graph_sequencer {
                    match elem {
                        Expression::Unquote(inner) => self.compile_expression(inner)?,
                        _ => self.compile_quoted_expression(elem)?,
                    }
                    continue;
                }
                if quote_next {
                    if quote_next_preserving {
                        self.compile_quoted_expression_preserving_quotes(elem)?;
                    } else {
                        self.compile_quoted_expression(elem)?;
                    }
                    quote_next = false;
                    quote_next_preserving = false;
                    continue;
                }
                if is_def_accumulator && list.len() == 3 && i == 1 {
                    self.compile_quoted_expression(elem)?;
                    continue;
                }
                if is_process_sugar || is_def_song {
                    self.compile_quoted_expression(elem)?;
                    continue;
                }
                match elem {
                    Expression::Number(c) => {
                        let constant_idx = self.use_constant(*c);
                        self.emit(OpCode::PushConst(constant_idx));
                    }
                    Expression::String(s) => {
                        let str_idx = self.use_string_constant(s);
                        self.emit(OpCode::PushStr(str_idx));
                    }
                    Expression::Symbol(c) => match op {
                        Expression::Symbol(s) if s == "def" && i == 0 => continue,
                        Expression::Symbol(_)
                            if (is_def_process || is_def_accumulator || is_defchan) && i == 0 =>
                        {
                            // Registry auto-qualification (spec §5):
                            // def-process/def-accumulator class names inside
                            // a declared module qualify, so the registry
                            // entry and the constructor native land under
                            // the module's name. defchan channel names stay
                            // flat for now (channels are a separate,
                            // handle-first keyspace).
                            let name = if is_def_process || is_def_accumulator {
                                self.qualify_registration_name(c)
                            } else {
                                c.clone()
                            };
                            let idx = self.use_string_constant(&name);
                            self.emit(OpCode::PushSymbol(idx));
                        }
                        _ => {
                            self.compile_expression(&Expression::Symbol(c.clone()))?;
                        }
                    },
                    Expression::List(l) => {
                        self.compile_list(l)?;
                    }
                    Expression::Keyword(k) => {
                        let idx = self.use_string_constant(k);
                        self.emit(OpCode::PushKeyword(idx));
                        if k == "material" || k == "shader" {
                            quote_next = true;
                        }
                        if is_def_sequencer && (k == "tick" || k == "init") {
                            quote_next = true;
                            quote_next_preserving = true;
                        }
                        if is_def_process
                            && (k == "in"
                                || k == "out"
                                || k == "state"
                                || k == "every"
                                || k == "phase"
                                || k == "listen"
                                || k == "target"
                                || k == "targets"
                                || k == "seed"
                                || k == "doc"
                                || k == "run"
                                || k == "init"
                                || k.starts_with("on-"))
                        {
                            quote_next = true;
                        }
                        if is_def_accumulator
                            && (k == "target"
                                || k == "amount"
                                || k == "reset"
                                || k == "range"
                                || k == "mode"
                                || k == "seed"
                                || k == "doc")
                        {
                            quote_next = true;
                        }
                    }
                    Expression::QuoteSymbol(s) => {
                        let idx = self.use_string_constant(s);
                        self.emit(OpCode::PushSymbol(idx));
                    }
                    Expression::QuoteList(items) => {
                        for item in items {
                            self.compile_quoted_expression(item)?;
                        }
                        self.emit(OpCode::MakeList(items.len()));
                    }
                    Expression::Quasiquote(_)
                    | Expression::Unquote(_)
                    | Expression::UnquoteSplicing(_) => {
                        self.compile_expression(elem)?;
                    }
                }
            }
            let arity = list.len() - 1;
            match op {
                Expression::Symbol(s) if s == "+" => self.emit(OpCode::Add(arity)),
                Expression::Symbol(s) if s == "*" => self.emit(OpCode::Mul(arity)),
                Expression::Symbol(s) if s == "-" => self.emit(OpCode::Sub(arity)),
                Expression::Symbol(s) if s == "/" => self.emit(OpCode::Div(arity)),
                Expression::Symbol(s) if s == "min" => self.emit(OpCode::Min(arity)),
                Expression::Symbol(s) if s == "max" => self.emit(OpCode::Max(arity)),
                Expression::Symbol(s) if s == "list" => self.emit(OpCode::MakeList(arity)),
                Expression::Symbol(s) if s == "=" => self.emit(OpCode::Eq),
                Expression::Symbol(s) if s == "<" => self.emit(OpCode::Lt),
                Expression::Symbol(s) if s == ">" => self.emit(OpCode::Gt),
                Expression::Symbol(s) if s == "<=" => self.emit(OpCode::Lte),
                Expression::Symbol(s) if s == ">=" => self.emit(OpCode::Gte),
                Expression::Symbol(s) if s == "def" => {
                    if let Some(Expression::Symbol(s)) = list.get(1) {
                        self.emit_symbol_store_for_definition(s);
                        self.emit(OpCode::PushNil);
                    }
                }
                _ => {
                    self.compile_expression(op)?;
                    self.emit(OpCode::Call(arity));
                }
            }
        }
        Ok(())
    }

    fn compile_expression(&mut self, expression: &Expression) -> Result<(), CompilerError> {
        match expression {
            Expression::List(l) => {
                self.compile_list(l)?;
            }
            Expression::Symbol(s) => {
                if s == "true" {
                    self.emit(OpCode::PushBool(true));
                    return Ok(());
                }
                if s == "false" {
                    self.emit(OpCode::PushBool(false));
                    return Ok(());
                }
                if s == "nil" {
                    self.emit(OpCode::PushNil);
                    return Ok(());
                }
                if let Some(node_id) = self.derived_bindings.get(s).copied() {
                    self.emit(OpCode::LoadDerived(node_id));
                    return Ok(());
                }
                // Dot syntax: person.age  →  load person, GetField("age")
                // person.address.city  →  load person, GetField("address"), GetField("city")
                // Module-qualified symbols (`test.mod/foo`) are exempt: the
                // `/` split wins and dots before it are module segments.
                let parts: Vec<&str> = s.splitn(2, '.').collect();
                if parts.len() == 2
                    && !super::modules::is_qualified(s)
                    && !parts[0].is_empty()
                    && !parts[1].is_empty()
                {
                    let fields = parts[1].split('.').collect::<Vec<_>>();
                    if self.reactive_namespaces.contains(parts[0]) {
                        let ns_idx = self.use_string_constant(parts[0]);
                        let field_idx = self.use_string_constant(fields[0]);
                        self.emit(OpCode::LoadReactive(ns_idx, field_idx));
                    } else {
                        self.emit_symbol_load(parts[0]);
                        let idx = self.use_string_constant(fields[0]);
                        self.emit(OpCode::GetField(idx));
                    }
                    for field in fields.into_iter().skip(1) {
                        let idx = self.use_string_constant(field);
                        self.emit(OpCode::GetField(idx));
                    }
                } else {
                    self.emit_symbol_load(s);
                }
            }
            Expression::Keyword(s) => {
                let idx = self.use_string_constant(s);
                self.emit(OpCode::PushKeyword(idx));
            }
            Expression::Number(n) => {
                let constant_idx = self.use_constant(*n);
                self.emit(OpCode::PushConst(constant_idx));
            }
            Expression::String(s) => {
                let str_idx = self.use_string_constant(s);
                self.emit(OpCode::PushStr(str_idx));
            }
            Expression::QuoteSymbol(s) => {
                let idx = self.use_string_constant(s);
                self.emit(OpCode::PushSymbol(idx));
            }
            Expression::QuoteList(items) => {
                for item in items {
                    self.compile_quoted_expression(item)?;
                }
                self.emit(OpCode::MakeList(items.len()));
            }
            Expression::Quasiquote(inner) => {
                // Outside of macro context, quasiquote behaves like quote
                self.compile_quoted_expression(inner)?;
            }
            Expression::Unquote(_) | Expression::UnquoteSplicing(_) => {
                // Unquote outside quasiquote is an error.
                return Err(CompilerError::InvalidArg);
            }
        }

        Ok(())
    }

    pub fn compile(&mut self) -> Result<Vec<Chunk>, CompilerError> {
        // A continuation segment (module context injected before compile,
        // see `set_module_context`) already belongs to its unit's module;
        // stamp the entry chunk the way the `(module …)` form re-stamps it
        // in the segment that actually contains the declaration.
        let entry_module = self.declared_module();
        _ = self.new_chunk(Chunk {
            ops: vec![],
            constants: vec![],
            strings: vec![],
            symbols: vec![],
            upvalues: vec![],
            source_symbol: None,
            source_file: self.source_file.clone(),
            source_module: entry_module,
        });
        let expressions = std::mem::take(&mut self.expressions);
        for expression in &expressions {
            self.compile_expression(expression)?;
        }
        if !self.errors.is_empty() {
            return Err(CompilerError::Message(self.errors.join("; ")));
        }
        Ok(std::mem::take(&mut self.chunks))
    }
}
