//! Minimal, tolerant s-expression reader for authored dgenlisp sources.
//! Only distinguishes atoms from lists; `;` comments are stripped, string
//! literals are kept as atoms with a leading `"` so symbol collection can
//! skip them. `[` / `]` parse as lists (tensor shapes).

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sexpr {
    Atom(String),
    List(Vec<Sexpr>),
}

impl Sexpr {
    pub fn atom(&self) -> Option<&str> {
        match self {
            Sexpr::Atom(a) => Some(a.as_str()),
            Sexpr::List(_) => None,
        }
    }
}

enum Token {
    Open,
    Close,
    Atom(String),
}

fn tokenize(source: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ';' => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        break;
                    }
                }
            }
            '(' | '[' => tokens.push(Token::Open),
            ')' | ']' => tokens.push(Token::Close),
            '"' => {
                let mut s = String::from('"');
                while let Some(n) = chars.next() {
                    if n == '\\' {
                        if let Some(escaped) = chars.next() {
                            s.push(escaped);
                        }
                    } else if n == '"' {
                        break;
                    } else {
                        s.push(n);
                    }
                }
                tokens.push(Token::Atom(s));
            }
            c if c.is_whitespace() => {}
            c => {
                let mut s = String::from(c);
                while let Some(&n) = chars.peek() {
                    if n.is_whitespace() || matches!(n, '(' | ')' | '[' | ']' | ';' | '"') {
                        break;
                    }
                    s.push(n);
                    chars.next();
                }
                tokens.push(Token::Atom(s));
            }
        }
    }
    tokens
}

fn parse_one(tokens: &[Token], pos: &mut usize) -> Option<Sexpr> {
    match tokens.get(*pos)? {
        Token::Atom(a) => {
            *pos += 1;
            Some(Sexpr::Atom(a.clone()))
        }
        Token::Close => {
            // Stray close at top level: skip it.
            *pos += 1;
            None
        }
        Token::Open => {
            *pos += 1;
            let mut items = Vec::new();
            while let Some(token) = tokens.get(*pos) {
                match token {
                    Token::Close => {
                        *pos += 1;
                        return Some(Sexpr::List(items));
                    }
                    _ => {
                        if let Some(item) = parse_one(tokens, pos) {
                            items.push(item);
                        }
                    }
                }
            }
            // Unbalanced open: keep what parsed.
            Some(Sexpr::List(items))
        }
    }
}

pub fn parse(source: &str) -> Vec<Sexpr> {
    let tokens = tokenize(source);
    let mut pos = 0;
    let mut forms = Vec::new();
    while pos < tokens.len() {
        if let Some(form) = parse_one(&tokens, &mut pos) {
            forms.push(form);
        }
    }
    forms
}
