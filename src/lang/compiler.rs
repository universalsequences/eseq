use crate::parser::Expression;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct Chunk {
    pub ops: Vec<OpCode>,
    pub constants: Vec<f64>,
    pub strings: Vec<String>, // string constants pool
    pub symbols: Vec<String>,
    pub upvalues: Vec<String>,
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
}

#[derive(Debug, Clone)]
pub enum OpCode {
    Push,
    PushConst(usize), // const idx
    PushStr(usize),   // string const idx
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
    LoadReactive(usize, usize),  // namespace idx, field idx
    StoreReactive(usize, usize), // namespace idx, field idx
    GetField(usize),             // pop a map, push map[strings[idx]]
    EmitTree,                    // pop widget tree from stack and route it to the runtime
    Return,
    Jump(usize),
    JumpIfFalse(usize),
    PushBool(bool),
    PushNil,
}

#[derive(Debug, Clone)]
pub struct MacroDef {
    pub params: Vec<String>,
    pub body: Expression,
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
    pub macros: HashMap<String, MacroDef>,
}

fn is_widget_name(name: &str) -> bool {
    matches!(
        name,
        "label"
            | "slider"
            | "hslider"
            | "vslider"
            | "toggle"
            | "knob"
            | "meter"
            | "text-input"
            | "select"
            | "v-stack"
            | "h-stack"
            | "box"
            | "grid"
            | "tabs"
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
            macros,
        }
    }

    pub fn macros(&self) -> &HashMap<String, MacroDef> {
        &self.macros
    }

    fn alloc_temp_symbol(&mut self, prefix: &str) -> String {
        let symbol = format!("__eseq_{}_{}", prefix, self.next_temp_id);
        self.next_temp_id += 1;
        symbol
    }

    pub fn expand_macros(&self, expr: &Expression, depth: usize) -> Expression {
        if depth > 100 {
            return expr.clone();
        }
        match expr {
            Expression::List(items) if !items.is_empty() => {
                if let Expression::Symbol(name) = &items[0] {
                    if let Some(mac) = self.macros.get(name) {
                        if items.len() - 1 == mac.params.len() {
                            // Build parameter bindings: expand macro args first
                            let mut bindings = HashMap::new();
                            for (param, arg) in mac.params.iter().zip(items.iter().skip(1)) {
                                bindings.insert(param.clone(), self.expand_macros(arg, depth + 1));
                            }
                            let expanded = Self::expand_quasiquote(&mac.body, &bindings);
                            return self.expand_macros(&expanded, depth + 1);
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

    fn expand_quasiquote(expr: &Expression, bindings: &HashMap<String, Expression>) -> Expression {
        match expr {
            Expression::Quasiquote(inner) => Self::expand_quasiquote_inner(inner, bindings),
            // If the macro body isn't quasiquoted, just return it as-is
            _ => expr.clone(),
        }
    }

    fn expand_quasiquote_inner(
        expr: &Expression,
        bindings: &HashMap<String, Expression>,
    ) -> Expression {
        match expr {
            Expression::Unquote(inner) => {
                // Substitute if the unquoted expression is a bound parameter
                if let Expression::Symbol(name) = inner.as_ref() {
                    if let Some(replacement) = bindings.get(name) {
                        return replacement.clone();
                    }
                }
                // Not a bound parameter — return inner as-is
                *inner.clone()
            }
            Expression::List(items) => Expression::List(
                items
                    .iter()
                    .map(|item| Self::expand_quasiquote_inner(item, bindings))
                    .collect(),
            ),
            // Everything else inside quasiquote is literal
            _ => expr.clone(),
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
            Expression::Unquote(inner) => {
                self.compile_quoted_expression(inner)?;
            }
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
        self.emit_symbol_store(name);
        Ok(())
    }

    fn compile_named_state_definition(
        &mut self,
        name: &str,
        initial: &Expression,
    ) -> Result<(), CompilerError> {
        let node_id = self
            .state_bindings
            .get(name)
            .copied()
            .unwrap_or_else(|| self.alloc_node_id());
        self.state_bindings.insert(name.to_string(), node_id);
        self.compile_expression(initial)?;
        self.emit(OpCode::InitState(node_id));
        let global_idx = self.use_global(name);
        self.emit(OpCode::StoreGlobal(global_idx));
        Ok(())
    }

    fn compile_set_statement(
        &mut self,
        target: &Expression,
        value: &Expression,
    ) -> Result<(), CompilerError> {
        self.compile_expression(value)?;

        match target {
            Expression::Symbol(name) => {
                let parts = name.splitn(2, '.').collect::<Vec<_>>();
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
            Expression::Quasiquote(_) | Expression::Unquote(_) => Err(CompilerError::InvalidArg),
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
        if let Some(index) = self.global_symbols.iter().position(|r| *r == name) {
            return index;
        }
        let idx = self.global_symbols.len();
        self.global_symbols.push(name.to_string());
        idx
    }

    pub fn new_chunk(&mut self, chunk: Chunk) -> (usize, usize) {
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
            SymbolResolution::Global(_) if self.state_bindings.contains_key(name) => {
                self.emit(OpCode::LoadState(self.state_bindings[name]))
            }
            SymbolResolution::Local(idx) => self.emit(OpCode::LoadLocal(idx)),
            SymbolResolution::Global(idx) => self.emit(OpCode::LoadGlobal(idx)),
            SymbolResolution::Upvalue(idx) => self.emit(OpCode::LoadUpvalue(idx)),
        }
    }

    fn emit_symbol_store(&mut self, name: &str) {
        match self.resolve_symbol(name) {
            SymbolResolution::Global(_) if self.state_bindings.contains_key(name) => {
                self.emit(OpCode::StoreState(self.state_bindings[name]))
            }
            SymbolResolution::Local(idx) => self.emit(OpCode::StoreLocal(idx)),
            SymbolResolution::Global(idx) => self.emit(OpCode::StoreGlobal(idx)),
            SymbolResolution::Upvalue(idx) => self.emit(OpCode::StoreUpvalue(idx)),
        }
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
            self.emit_symbol_store(&name);
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
            if self.macros.contains_key(name) {
                let call_expr = Expression::List(list.to_vec());
                let expanded = self.expand_macros(&call_expr, 0);
                return self.compile_expression(&expanded);
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
            if s == "defmacro" && list.len() == 4 {
                let Expression::Symbol(name) = &list[1] else {
                    return Err(CompilerError::InvalidArg);
                };
                let Expression::List(params_expr) = &list[2] else {
                    return Err(CompilerError::InvalidArg);
                };
                let params: Vec<String> = params_expr
                    .iter()
                    .map(|p| match p {
                        Expression::Symbol(s) => Ok(s.clone()),
                        _ => Err(CompilerError::InvalidArg),
                    })
                    .collect::<Result<_, _>>()?;
                self.macros.insert(
                    name.clone(),
                    MacroDef {
                        params,
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
                        if (key == "shader" || key == "state") && i + 1 < list.len() {
                            // Auto-quote shader and state expressions
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
            let mut quote_next = false;
            for (i, elem) in list.iter().skip(1).enumerate() {
                if quote_next {
                    self.compile_quoted_expression(elem)?;
                    quote_next = false;
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
                    Expression::Quasiquote(_) | Expression::Unquote(_) => {
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
                        self.emit_symbol_store(s);
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
                let parts: Vec<&str> = s.splitn(2, '.').collect();
                if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
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
            Expression::Unquote(_) => {
                // Unquote outside quasiquote is an error
                return Err(CompilerError::InvalidArg);
            }
        }

        Ok(())
    }

    pub fn compile(&mut self) -> Result<Vec<Chunk>, CompilerError> {
        _ = self.new_chunk(Chunk {
            ops: vec![],
            constants: vec![],
            strings: vec![],
            symbols: vec![],
            upvalues: vec![],
        });
        let expressions = std::mem::take(&mut self.expressions);
        for expression in &expressions {
            self.compile_expression(expression)?;
        }
        Ok(std::mem::take(&mut self.chunks))
    }
}
