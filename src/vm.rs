use crate::compiler::{Chunk, Compiler, OpCode};
use crate::parser::{ASTParser, Parser};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::rc::Rc;

static RAND_STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);

#[derive(Debug, PartialEq)]
pub enum VMError {
    UnknownConstant,
    UnknownOpcode,
    StackUnderflow,
    IncorrectType,
    UnknownVariable(String),
    ExpectedFunction,
    ArityMismatch,
    ParseError,
    CompileError,
}

pub type NativeFn = Rc<dyn Fn(Vec<Value>) -> Value>;

pub enum Value {
    Number(f64),
    Bool(bool),
    Nil,
    String(String),
    Symbol(String),
    Keyword(String),
    List(Vec<Rc<RefCell<Value>>>),
    Map(HashMap<String, Rc<RefCell<Value>>>),
    Closure(usize, Vec<Rc<RefCell<Value>>>),
    Function(usize),
    NativeFunction(NativeFn),
}

pub fn format_lisp_value(value: &Value) -> String {
    match value {
        Value::Number(n) => {
            if n.fract() == 0.0 {
                format!("{:.0}", n)
            } else {
                format!("{n}")
            }
        }
        Value::Bool(b) => b.to_string(),
        Value::Nil => "nil".to_string(),
        Value::String(s) => format!("{s:?}"),
        Value::Symbol(s) => format!("'{s}"),
        Value::Keyword(s) => format!(":{s}"),
        Value::List(items) => {
            let rendered = items
                .iter()
                .map(|item| format_lisp_value(&item.borrow()))
                .collect::<Vec<_>>()
                .join(" ");
            format!("({rendered})")
        }
        Value::Map(map) => {
            let mut entries = map
                .iter()
                .map(|(key, value)| (key.clone(), format_lisp_value(&value.borrow())))
                .collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let rendered = entries
                .into_iter()
                .map(|(key, value)| format!(":{key} {value}"))
                .collect::<Vec<_>>()
                .join(" ");
            format!("{{{rendered}}}")
        }
        Value::Closure(i, _) => format!("<closure:{i}>"),
        Value::Function(i) => format!("<fn:{i}>"),
        Value::NativeFunction(_) => "<native>".to_string(),
    }
}

pub fn format_lisp_source(value: &Value) -> String {
    match value {
        Value::Number(n) => {
            if n.fract() == 0.0 {
                format!("{:.0}", n)
            } else {
                format!("{n}")
            }
        }
        Value::Bool(b) => b.to_string(),
        Value::Nil => "nil".to_string(),
        Value::String(s) => format!("{s:?}"),
        Value::Symbol(s) => s.clone(),
        Value::Keyword(s) => format!(":{s}"),
        Value::List(items) => {
            let rendered = items
                .iter()
                .map(|item| format_lisp_source(&item.borrow()))
                .collect::<Vec<_>>()
                .join(" ");
            format!("({rendered})")
        }
        Value::Map(map) => {
            let mut entries = map
                .iter()
                .map(|(key, value)| (key.clone(), format_lisp_source(&value.borrow())))
                .collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let rendered = entries
                .into_iter()
                .map(|(key, value)| format!(":{key} {value}"))
                .collect::<Vec<_>>()
                .join(" ");
            format!("(dict {rendered})")
        }
        Value::Closure(i, _) => format!("<closure:{i}>"),
        Value::Function(i) => format!("<fn:{i}>"),
        Value::NativeFunction(_) => "<native>".to_string(),
    }
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", format_lisp_value(self))
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => a == b,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Nil, Self::Nil) => true,
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Symbol(a), Self::Symbol(b)) => a == b,
            (Self::Keyword(a), Self::Keyword(b)) => a == b,
            (Self::List(a), Self::List(b)) => {
                a.len() == b.len()
                    && a.iter().zip(b.iter()).all(|(x, y)| *x.borrow() == *y.borrow())
            }
            (Self::Closure(a, _), Self::Closure(b, _)) => a == b,
            (Self::Function(a), Self::Function(b)) => a == b,
            _ => false,
        }
    }
}

impl Clone for Value {
    fn clone(&self) -> Self {
        match self {
            Self::Number(n) => Self::Number(*n),
            Self::Bool(b) => Self::Bool(*b),
            Self::Nil => Self::Nil,
            Self::String(s) => Self::String(s.clone()),
            Self::Symbol(s) => Self::Symbol(s.clone()),
            Self::Keyword(s) => Self::Keyword(s.clone()),
            Self::List(l) => Self::List(l.clone()),
            Self::Map(m) => Self::Map(m.clone()),
            Self::Closure(i, u) => Self::Closure(*i, u.clone()),
            Self::Function(i) => Self::Function(*i),
            Self::NativeFunction(f) => Self::NativeFunction(f.clone()),
        }
    }
}

struct Frame {
    locals: Vec<Option<Rc<RefCell<Value>>>>,
    upvalues: Vec<Rc<RefCell<Value>>>,
    pc: usize,
    chunk_idx: usize,
}

pub struct VM {
    pub chunks: Vec<Chunk>,
    current_chunk: usize,
    globals: Vec<Option<Rc<RefCell<Value>>>>,
    pub global_names: Vec<String>,
}

/// Register built-in functions available in all contexts.
pub fn register_core_natives(vm: &mut VM) {
    // (dict :key val :key val ...) → Map
    vm.register_native("dict", |args| {
        let mut map = HashMap::new();
        let mut i = 0;
        while i + 1 < args.len() {
            if let Value::Keyword(k) = &args[i] {
                map.insert(k.clone(), Rc::new(RefCell::new(args[i + 1].clone())));
            }
            i += 2;
        }
        Value::Map(map)
    });

    // (get map :key) → value, or false if missing
    vm.register_native("get", |args| {
        if let (Some(Value::Map(m)), Some(Value::Keyword(k))) = (args.first(), args.get(1)) {
            m.get(k).map(|v| v.borrow().clone()).unwrap_or(Value::Nil)
        } else {
            Value::Nil
        }
    });

    // (merge map :key val ...) → new map with overrides
    vm.register_native("merge", |args| {
        let mut map = if let Some(Value::Map(m)) = args.first() {
            m.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<HashMap<_, _>>()
        } else {
            HashMap::new()
        };
        let mut i = 1;
        while i + 1 < args.len() {
            if let Value::Keyword(k) = &args[i] {
                map.insert(k.clone(), Rc::new(RefCell::new(args[i + 1].clone())));
            }
            i += 2;
        }
        Value::Map(map)
    });

    // (keys map) → List of keywords
    vm.register_native("keys", |args| {
        if let Some(Value::Map(m)) = args.first() {
            Value::List(
                m.keys()
                    .map(|k| Rc::new(RefCell::new(Value::Keyword(k.clone()))))
                    .collect(),
            )
        } else {
            Value::Nil
        }
    });

    // (first list) → first element or false
    vm.register_native("first", |args| {
        if let Some(Value::List(l)) = args.first() {
            l.first().map(|v| v.borrow().clone()).unwrap_or(Value::Nil)
        } else {
            Value::Nil
        }
    });

    // (rest list) → tail of list or empty list
    vm.register_native("rest", |args| {
        if let Some(Value::List(l)) = args.first() {
            Value::List(l[1..].to_vec())
        } else {
            Value::List(vec![])
        }
    });

    // (cons val list) → new list with val prepended
    vm.register_native("cons", |args| {
        if let (Some(head), Some(Value::List(tail))) = (args.first(), args.get(1)) {
            let mut new = vec![Rc::new(RefCell::new(head.clone()))];
            new.extend(tail.iter().cloned());
            Value::List(new)
        } else {
            Value::List(vec![])
        }
    });

    // (len list-or-string) → number
    vm.register_native("len", |args| match args.first() {
        Some(Value::List(l)) => Value::Number(l.len() as f64),
        Some(Value::String(s)) => Value::Number(s.len() as f64),
        _ => Value::Number(0.0),
    });

    // (append list ...) → concatenated list
    vm.register_native("append", |args| {
        let mut result = vec![];
        for arg in &args {
            if let Value::List(l) = arg {
                result.extend(l.iter().cloned());
            }
        }
        Value::List(result)
    });

    // (list a b c) -> List
    vm.register_native("list", |args| {
        Value::List(
            args.into_iter()
                .map(|value| Rc::new(RefCell::new(value)))
                .collect(),
        )
    });

    // (nth list idx) -> value or nil; idx is 0-based
    vm.register_native("nth", |args| {
        let (Some(Value::List(list)), Some(Value::Number(idx))) = (args.first(), args.get(1)) else {
            return Value::Nil;
        };
        if *idx < 0.0 {
            return Value::Nil;
        }
        list.get(*idx as usize)
            .map(|value| value.borrow().clone())
            .unwrap_or(Value::Nil)
    });

    // (reverse list) -> reversed list
    vm.register_native("reverse", |args| {
        let Some(Value::List(list)) = args.first() else {
            return Value::List(vec![]);
        };
        let mut result = list.clone();
        result.reverse();
        Value::List(result)
    });

    // (range end) or (range start end) -> list of numbers
    vm.register_native("range", |args| {
        let (start, end) = match args.as_slice() {
            [Value::Number(end)] => (0_i64, *end as i64),
            [Value::Number(start), Value::Number(end)] => (*start as i64, *end as i64),
            _ => return Value::List(vec![]),
        };

        let mut values = Vec::new();
        if start <= end {
            for n in start..end {
                values.push(Rc::new(RefCell::new(Value::Number(n as f64))));
            }
        } else {
            for n in (end + 1..=start).rev() {
                values.push(Rc::new(RefCell::new(Value::Number(n as f64))));
            }
        }
        Value::List(values)
    });

    // (rand-int end) or (rand-int start end) -> integer in [0,end) or [start,end)
    vm.register_native("rand-int", |args| {
        let (start, end) = match args.as_slice() {
            [Value::Number(end)] => (0_i64, *end as i64),
            [Value::Number(start), Value::Number(end)] => (*start as i64, *end as i64),
            _ => return Value::Nil,
        };

        if end <= start {
            return Value::Nil;
        }

        let span = (end - start) as u64;
        let mut state = RAND_STATE.load(Ordering::Relaxed);
        loop {
            let next = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            match RAND_STATE.compare_exchange_weak(
                state,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let value = start + (next % span) as i64;
                    return Value::Number(value as f64);
                }
                Err(current) => state = current,
            }
        }
    });

    // (not val) → bool
    vm.register_native("not", |args| {
        Value::Bool(matches!(
            args.first(),
            Some(Value::Bool(false)) | Some(Value::Nil) | None
        ))
    });

    // (str val ...) → concatenated Lisp string representation
    vm.register_native("str", |args| {
        let mut s = String::new();
        for v in &args {
            s.push_str(&format_lisp_value(v));
        }
        Value::String(s)
    });

    // (source val ...) → concatenated evaluable Lisp source
    vm.register_native("source", |args| {
        let mut s = String::new();
        for v in &args {
            s.push_str(&format_lisp_source(v));
        }
        Value::String(s)
    });
}

impl VM {
    pub fn new(chunks: Vec<Chunk>) -> Self {
        VM {
            chunks,
            current_chunk: 0,
            globals: vec![None; 512],
            global_names: vec![],
        }
    }

    /// Register a Rust function as a named global callable from Lisp.
    pub fn register_native(&mut self, name: &str, f: impl Fn(Vec<Value>) -> Value + 'static) {
        let idx = self.ensure_global(name);
        self.globals[idx] = Some(Rc::new(RefCell::new(Value::NativeFunction(Rc::new(f)))));
    }

    /// Compile and run `code` in this VM's existing context (globals persist).
    pub fn eval_str(&mut self, code: &str) -> Result<Option<Value>, VMError> {
        let tokens = Parser::new(code.to_string())
            .parse()
            .map_err(|_| VMError::ParseError)?;
        let exprs = ASTParser::new(tokens)
            .parse()
            .map_err(|_| VMError::ParseError)?;

        let entry_idx = self.chunks.len();
        let existing = self.chunks.clone();
        let names = self.global_names.clone();

        let mut compiler = Compiler::new_repl(exprs, existing, names);
        match compiler.compile() {
            Ok(chunks) => {
                self.chunks = chunks;
                self.global_names = compiler.into_global_names();
                self.execute_from(entry_idx)
            }
            Err(_) => Err(VMError::CompileError),
        }
    }

    fn ensure_global(&mut self, name: &str) -> usize {
        if let Some(idx) = self.global_names.iter().position(|n| n == name) {
            return idx;
        }
        let idx = self.global_names.len();
        self.global_names.push(name.to_string());
        idx
    }

    pub fn set_global_value(&mut self, name: &str, value: Value) {
        let idx = self.ensure_global(name);
        if idx >= self.globals.len() {
            self.globals.resize(idx + 1, None);
        }
        self.globals[idx] = Some(Rc::new(RefCell::new(value)));
    }

    pub fn global_names(&self) -> &[String] {
        &self.global_names
    }

    pub fn global_value(&self, name: &str) -> Option<Value> {
        let idx = self.global_names.iter().position(|global| global == name)?;
        self.globals
            .get(idx)
            .and_then(|value| value.as_ref())
            .map(|value| value.borrow().clone())
    }

    fn execute_from(&mut self, entry_chunk: usize) -> Result<Option<Value>, VMError> {
        self.current_chunk = entry_chunk;
        self.execute()
    }

    fn chunk(&self) -> &Chunk {
        self.chunks.get(self.current_chunk).unwrap()
    }

    fn new_frame(&self) -> Frame {
        Frame {
            locals: vec![None; self.chunk().symbols.len()],
            upvalues: vec![],
            pc: 0,
            chunk_idx: self.current_chunk,
        }
    }

    fn unknown_local(&self, frame: &Frame, idx: usize) -> VMError {
        let name = self
            .chunks
            .get(frame.chunk_idx)
            .and_then(|chunk| chunk.symbols.get(idx))
            .cloned()
            .unwrap_or_else(|| format!("<local:{idx}>"));
        VMError::UnknownVariable(name)
    }

    fn unknown_upvalue(&self, frame: &Frame, idx: usize) -> VMError {
        let name = self
            .chunks
            .get(frame.chunk_idx)
            .and_then(|chunk| chunk.upvalues.get(idx))
            .cloned()
            .unwrap_or_else(|| format!("<upvalue:{idx}>"));
        VMError::UnknownVariable(name)
    }

    fn unknown_global(&self, idx: usize) -> VMError {
        let name = self
            .global_names
            .get(idx)
            .cloned()
            .unwrap_or_else(|| format!("<global:{idx}>"));
        VMError::UnknownVariable(name)
    }

    pub fn execute(&mut self) -> Result<Option<Value>, VMError> {
        let mut stack: Vec<Rc<RefCell<Value>>> = vec![];
        let mut frames: Vec<Frame> = vec![self.new_frame()];

        while frames.last().unwrap().pc < self.chunks[self.current_chunk].ops.len() {
            let op = self.chunks[self.current_chunk].ops[frames.last().unwrap().pc].clone();
            match op {
                OpCode::PushConst(x) => {
                    if let Some(constant) = self.chunks[self.current_chunk].constants.get(x) {
                        stack.push(Rc::new(RefCell::new(Value::Number(*constant))));
                        frames.last_mut().unwrap().pc += 1;
                    } else {
                        return Err(VMError::UnknownConstant);
                    }
                }
                OpCode::PushStr(x) => {
                    if let Some(s) = self.chunks[self.current_chunk].strings.get(x) {
                        stack.push(Rc::new(RefCell::new(Value::String(s.clone()))));
                        frames.last_mut().unwrap().pc += 1;
                    } else {
                        return Err(VMError::UnknownConstant);
                    }
                }
                OpCode::PushBool(value) => {
                    stack.push(Rc::new(RefCell::new(Value::Bool(value))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::PushNil => {
                    stack.push(Rc::new(RefCell::new(Value::Nil)));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Add(arity) => {
                    if stack.len() < arity {
                        return Err(VMError::StackUnderflow);
                    }
                    let mut sum: f64 = 0.0;
                    for _ in 0..arity {
                        if let Some(val) = stack.pop() {
                            match *val.borrow() {
                                Value::Number(val) => sum += val,
                                _ => return Err(VMError::IncorrectType),
                            }
                        }
                    }
                    stack.push(Rc::new(RefCell::new(Value::Number(sum))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Sub(arity) => {
                    if stack.len() < arity {
                        return Err(VMError::StackUnderflow);
                    }
                    let mut nums: Vec<f64> = vec![];
                    for _ in 0..arity {
                        if let Some(val) = stack.pop() {
                            match *val.borrow() {
                                Value::Number(val) => nums.push(val),
                                _ => return Err(VMError::IncorrectType),
                            }
                        }
                    }
                    nums.reverse();
                    let diff = nums[1..].iter().fold(nums[0], |acc, x| acc - x);
                    stack.push(Rc::new(RefCell::new(Value::Number(diff))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Mul(arity) => {
                    if stack.len() < arity {
                        return Err(VMError::StackUnderflow);
                    }
                    let mut product: f64 = 1.0;
                    for _ in 0..arity {
                        if let Some(val) = stack.pop() {
                            match *val.borrow() {
                                Value::Number(val) => product *= val,
                                _ => return Err(VMError::IncorrectType),
                            }
                        }
                    }
                    stack.push(Rc::new(RefCell::new(Value::Number(product))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Div(arity) => {
                    if stack.len() < arity || arity == 0 {
                        return Err(VMError::StackUnderflow);
                    }
                    let mut nums: Vec<f64> = vec![];
                    for _ in 0..arity {
                        if let Some(val) = stack.pop() {
                            match *val.borrow() {
                                Value::Number(val) => nums.push(val),
                                _ => return Err(VMError::IncorrectType),
                            }
                        }
                    }
                    nums.reverse();
                    let quotient = if nums.len() == 1 {
                        1.0 / nums[0]
                    } else {
                        nums[1..].iter().fold(nums[0], |acc, x| acc / x)
                    };
                    stack.push(Rc::new(RefCell::new(Value::Number(quotient))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Min(arity) => {
                    if stack.len() < arity || arity == 0 {
                        return Err(VMError::StackUnderflow);
                    }
                    let mut current = f64::INFINITY;
                    for _ in 0..arity {
                        if let Some(val) = stack.pop() {
                            match *val.borrow() {
                                Value::Number(val) => current = current.min(val),
                                _ => return Err(VMError::IncorrectType),
                            }
                        }
                    }
                    stack.push(Rc::new(RefCell::new(Value::Number(current))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Max(arity) => {
                    if stack.len() < arity || arity == 0 {
                        return Err(VMError::StackUnderflow);
                    }
                    let mut current = f64::NEG_INFINITY;
                    for _ in 0..arity {
                        if let Some(val) = stack.pop() {
                            match *val.borrow() {
                                Value::Number(val) => current = current.max(val),
                                _ => return Err(VMError::IncorrectType),
                            }
                        }
                    }
                    stack.push(Rc::new(RefCell::new(Value::Number(current))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Pop => {
                    if stack.pop().is_none() {
                        return Err(VMError::StackUnderflow);
                    }
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Eq => {
                    if stack.len() < 2 {
                        return Err(VMError::StackUnderflow);
                    }
                    let mut result: bool = false;
                    if let (Some(a), Some(b)) = (stack.pop(), stack.pop()) {
                        match (&*a.borrow(), &*b.borrow()) {
                            (Value::Number(a), Value::Number(b)) => result = a == b,
                            (Value::Bool(a), Value::Bool(b)) => result = a == b,
                            (Value::Nil, Value::Nil) => result = true,
                            (Value::String(a), Value::String(b)) => result = a == b,
                            _ => return Err(VMError::IncorrectType),
                        }
                    }
                    stack.push(Rc::new(RefCell::new(Value::Bool(result))));
                    frames.last_mut().unwrap().pc += 1;
                }
                op @ (OpCode::Lt | OpCode::Gt | OpCode::Lte | OpCode::Gte) => {
                    if stack.len() < 2 {
                        return Err(VMError::StackUnderflow);
                    }
                    if let (Some(a), Some(b)) = (stack.pop(), stack.pop()) {
                        match (&*a.borrow(), &*b.borrow()) {
                            (Value::Number(a), Value::Number(b)) => {
                                let result = match op {
                                    OpCode::Lt  => b < a,
                                    OpCode::Gt  => b > a,
                                    OpCode::Lte => b <= a,
                                    _           => b >= a,
                                };
                                stack.push(Rc::new(RefCell::new(Value::Bool(result))));
                            }
                            _ => return Err(VMError::IncorrectType),
                        }
                    }
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::MakeList(arity) => {
                    if stack.len() < arity {
                        return Err(VMError::StackUnderflow);
                    }
                    let mut list: Vec<_> = (0..arity).filter_map(|_| stack.pop()).collect();
                    list.reverse();
                    stack.push(Rc::new(RefCell::new(Value::List(list))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::Jump(pc) => {
                    if let Some(frame) = frames.last_mut() {
                        frame.pc += pc;
                    }
                }
                OpCode::JumpIfFalse(pc) => {
                    if stack.is_empty() {
                        return Err(VMError::StackUnderflow);
                    }
                    if let Some(result) = stack.pop()
                        && let Some(frame) = frames.last_mut()
                    {
                        let is_false = match &*result.borrow() {
                            Value::Bool(r) => !r,
                            Value::Nil => true,
                            Value::Number(r) => *r == 0.0,
                            Value::String(r) => r.is_empty(),
                            Value::List(r) => r.is_empty(),
                            _ => false,
                        };
                        if is_false {
                            frame.pc += pc;
                        } else {
                            frame.pc += 1;
                        }
                    }
                }
                OpCode::StoreLocal(idx) => {
                    if let Some(frame) = frames.last_mut() {
                        frame.locals[idx] = stack.pop();
                        frame.pc += 1;
                    }
                }
                OpCode::LoadLocal(idx) => {
                    if let Some(frame) = frames.last_mut() {
                        if let Some(Some(val)) = frame.locals.get(idx) {
                            stack.push(Rc::clone(val));
                            frame.pc += 1;
                        } else {
                            return Err(self.unknown_local(frame, idx));
                        }
                    }
                }
                OpCode::StoreUpvalue(idx) => {
                    if let Some(frame) = frames.last_mut() {
                        frame.upvalues[idx] = stack.pop().unwrap();
                        frame.pc += 1;
                    }
                }
                OpCode::LoadUpvalue(idx) => {
                    if let Some(frame) = frames.last_mut() {
                        if let Some(val) = frame.upvalues.get(idx) {
                            stack.push(Rc::clone(val));
                            frame.pc += 1;
                        } else {
                            return Err(self.unknown_upvalue(frame, idx));
                        }
                    }
                }
                OpCode::StoreGlobal(idx) => {
                    if let Some(frame) = frames.last_mut() {
                        self.globals[idx] = stack.pop();
                        frame.pc += 1;
                    }
                }
                OpCode::LoadGlobal(idx) => {
                    if let Some(frame) = frames.last_mut() {
                        if let Some(Some(val)) = self.globals.get(idx) {
                            stack.push(Rc::clone(val));
                            frame.pc += 1;
                        } else {
                            let _ = frame;
                            return Err(self.unknown_global(idx));
                        }
                    }
                }
                OpCode::MakeClosure(chunk_idx, num_upvalues) => {
                    if let Some(frame) = frames.last_mut() {
                        let mut upvalues = vec![];
                        for _ in 0..num_upvalues {
                            upvalues.push(stack.pop().unwrap());
                        }
                        upvalues.reverse();
                        stack.push(Rc::new(Value::Closure(chunk_idx, upvalues).into()));
                        frame.pc += 1;
                    }
                }
                OpCode::Call(arity) => {
                    if let Some(v) = stack.pop() {
                        let borrowed = v.borrow();
                        match &*borrowed {
                            Value::Closure(chunk_idx, upvalues) => {
                                let chunk_idx = *chunk_idx;
                                let upvalues = upvalues.clone();
                                drop(borrowed);
                                self.current_chunk = chunk_idx;
                                let mut frame = self.new_frame();
                                frame.upvalues = upvalues;
                                if arity > frame.locals.len() {
                                    return Err(VMError::ArityMismatch);
                                }
                                if stack.len() < arity {
                                    return Err(VMError::StackUnderflow);
                                }
                                for i in 0..arity {
                                    let local_idx = arity - i - 1;
                                    let Some(slot) = frame.locals.get_mut(local_idx) else {
                                        return Err(VMError::ArityMismatch);
                                    };
                                    *slot = stack.pop();
                                }
                                frames.last_mut().unwrap().pc += 1;
                                frames.push(frame);
                            }
                            Value::NativeFunction(f) => {
                                // Clone the Rc so we can drop the borrow before touching the stack
                                let f = f.clone();
                                drop(borrowed);
                                let mut args: Vec<Value> = (0..arity)
                                    .filter_map(|_| stack.pop())
                                    .map(|v| v.borrow().clone())
                                    .collect();
                                args.reverse();
                                let result = f(args);
                                stack.push(Rc::new(RefCell::new(result)));
                                frames.last_mut().unwrap().pc += 1;
                            }
                            _ => {
                                return Err(VMError::ExpectedFunction);
                            }
                        }
                    }
                }
                // TODO(human): implement OpCode::Eval
                //
                // At this point in the execute loop you have &mut self, so eval_str is safe to call.
                // Steps:
                //   1. Pop the top of the stack — it should be a Value::String(code)
                //   2. Save self.current_chunk (eval_str will overwrite it)
                //   3. Call self.eval_str(&code) — this compiles + runs the string, returns a Value
                //   4. Restore self.current_chunk to the saved value
                //   5. Push the result onto the stack (or Value::Bool(false) on None/error)
                //   6. Advance pc by 1
                OpCode::Eval => match stack.pop() {
                    Some(val) => {
                        if let Value::String(code) = &*(val.borrow()) {
                            let current_chunk = self.current_chunk;
                            match (self.eval_str(code)?, frames.last_mut()) {
                                (result, Some(frame)) => {
                                    self.current_chunk = current_chunk;
                                    stack.push(Rc::new(RefCell::new(
                                        result.unwrap_or(Value::Nil),
                                    )));
                                    frame.pc += 1;
                                }
                                _ => {
                                    return Err(VMError::IncorrectType);
                                }
                            }
                        }
                    }
                    None => {
                        return Err(VMError::StackUnderflow);
                    }
                },
                OpCode::PushKeyword(idx) => {
                    let kw = self.chunks[self.current_chunk].strings[idx].clone();
                    stack.push(Rc::new(RefCell::new(Value::Keyword(kw))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::PushSymbol(idx) => {
                    let sym = self.chunks[self.current_chunk].strings[idx].clone();
                    stack.push(Rc::new(RefCell::new(Value::Symbol(sym))));
                    frames.last_mut().unwrap().pc += 1;
                }
                OpCode::GetField(idx) => {
                    let key = self.chunks[self.current_chunk].strings[idx].clone();
                    match stack.pop() {
                        Some(val) => {
                            let result = match &*val.borrow() {
                                Value::Map(m) => m.get(&key).cloned()
                                    .unwrap_or_else(|| Rc::new(RefCell::new(Value::Nil))),
                                _ => return Err(VMError::IncorrectType),
                            };
                            stack.push(result);
                            frames.last_mut().unwrap().pc += 1;
                        }
                        None => return Err(VMError::StackUnderflow),
                    }
                }
                OpCode::Return => match stack.pop() {
                    Some(return_value) => {
                        frames.pop();
                        if let Some(caller_frame) = frames.last() {
                            self.current_chunk = caller_frame.chunk_idx;
                            stack.push(return_value);
                        } else {
                            // Last frame returned — this is the final result
                            return Ok(Some(return_value.borrow().clone()));
                        }
                    }
                    None => return Err(VMError::StackUnderflow),
                },
                _ => return Err(VMError::UnknownOpcode),
            }
        }

        if let Some(result) = stack.last() {
            return Ok(Some(result.borrow().clone()));
        }
        Ok(None)
    }
}
