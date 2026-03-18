use crate::parser::Expression;

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
    Eval,               // pop a string, eval it in the current VM context, push result
    PushKeyword(usize), // push Value::Keyword from strings pool
    PushSymbol(usize),  // push Value::Symbol from strings pool (quoted symbol)
    GetField(usize),    // pop a map, push map[strings[idx]]
    Return,
    Jump(usize),
    JumpIfFalse(usize),
    PushBool(bool),
    PushNil,
}

pub struct Compiler {
    expressions: Vec<Expression>,
    chunks: Vec<Chunk>,
    scopes: Vec<Scope>,
    current_chunk: usize,
    global_symbols: Vec<String>,
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

impl Compiler {
    pub fn new(expressions: Vec<Expression>) -> Self {
        Compiler {
            expressions,
            chunks: vec![],
            scopes: vec![],
            current_chunk: 0,
            global_symbols: vec![],
        }
    }

    /// For REPL/eval_str: start with existing chunks and global symbol table
    /// so new code compiles against the same indices.
    pub fn new_repl(
        expressions: Vec<Expression>,
        existing_chunks: Vec<Chunk>,
        existing_global_names: Vec<String>,
    ) -> Self {
        Compiler {
            expressions,
            chunks: existing_chunks,
            scopes: vec![],
            current_chunk: 0,
            global_symbols: existing_global_names,
        }
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
        }

        Ok(())
    }

    /// Consume the compiler and return the final global symbol table,
    /// so the VM can sync its own name→index mapping.
    pub fn into_global_names(self) -> Vec<String> {
        self.global_symbols
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

        if let Some(parent) = self.scopes.iter().rev().nth(1)
            && let Some(_) = parent
                .symbols
                .iter()
                .position(|s| *s == name)
                .or_else(|| parent.upvalues.iter().position(|s| *s == name))
        {
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
            SymbolResolution::Local(idx) => self.emit(OpCode::LoadLocal(idx)),
            SymbolResolution::Global(idx) => self.emit(OpCode::LoadGlobal(idx)),
            SymbolResolution::Upvalue(idx) => self.emit(OpCode::LoadUpvalue(idx)),
        }
    }

    fn emit_symbol_store(&mut self, name: &str) {
        match self.resolve_symbol(name) {
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
        let symbols: Vec<String> = args
            .iter()
            .map(|arg| {
                if let Expression::Symbol(s) = arg {
                    Ok(s.to_string())
                } else {
                    Err(CompilerError::InvalidArg)
                }
            })
            .collect::<Result<_, _>>()?;
        let (new_chunk_idx, previous_chunk_idx) = self.new_chunk(Chunk {
            ops: vec![],
            constants: vec![],
            strings: vec![],
            symbols,
            upvalues: vec![],
        });
        self.compile_block(&body)?;

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

        let mut args = Vec::with_capacity(bindings.len());
        let mut values = Vec::with_capacity(bindings.len());

        for binding in bindings {
            let Expression::List(pair) = binding else {
                return Err(CompilerError::InvalidArg);
            };
            if pair.len() != 2 {
                return Err(CompilerError::InvalidArg);
            }

            let Expression::Symbol(name) = &pair[0] else {
                return Err(CompilerError::InvalidArg);
            };

            args.push(Expression::Symbol(name.clone()));
            values.push(pair[1].clone());
        }

        for value in values {
            self.compile_expression(&value)?;
        }
        self.compile_function(None, args, body.to_vec())?;
        self.emit(OpCode::Call(bindings.len()));
        Ok(())
    }

    pub fn compile_list(&mut self, list: &[Expression]) -> Result<(), CompilerError> {
        if let Some((name, args, body)) = extract_function_definition(list) {
            return self.compile_function(name, args, body);
        }

        if let Some((cond, then_body, else_body)) = extract_if_statement(list) {
            return self.compile_if_statement(cond, then_body, else_body);
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
            if s == "do" {
                return self.compile_block(&list[1..]);
            }
            if s == "let" && list.len() >= 3 {
                return self.compile_let_statement(&list[1], &list[2..]);
            }
        }

        let op = list.first();

        if let Some(op) = op {
            for (i, elem) in list.iter().skip(1).enumerate() {
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
                // Dot syntax: person.age  →  load person, GetField("age")
                // person.address.city  →  load person, GetField("address"), GetField("city")
                let parts: Vec<&str> = s.splitn(2, '.').collect();
                if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
                    self.emit_symbol_load(parts[0]);
                    // Chain remaining fields (handles a.b.c via recursion of the rest)
                    for field in parts[1].split('.') {
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
