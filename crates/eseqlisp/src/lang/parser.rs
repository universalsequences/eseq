pub struct Parser {
    text: String,
    pos: usize,
    #[cfg(test)]
    profile: std::cell::RefCell<ParserProfile>,
}

#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub struct ParserProfile {
    pub input_bytes: usize,
    pub peek_calls: usize,
    pub next_calls: usize,
    pub peek_nth_calls: usize,
    pub estimated_char_visits: usize,
    pub parse_text_calls: usize,
    pub skip_whitespace_loops: usize,
    pub parse_symbol_calls: usize,
    pub parse_number_calls: usize,
    pub parse_string_calls: usize,
    pub comments_skipped: usize,
    pub tokens_emitted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceSpan {
    pub start_byte: usize,
    pub end_byte: usize,
}

impl SourceSpan {
    pub fn new(start_byte: usize, end_byte: usize) -> Self {
        Self {
            start_byte,
            end_byte,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceOrigin {
    pub source_id: Option<String>,
    pub revision: Option<u64>,
    pub primary_span: SourceSpan,
    pub expansion_chain: Vec<SourceSpan>,
}

impl SourceOrigin {
    pub fn new(primary_span: SourceSpan) -> Self {
        Self {
            source_id: None,
            revision: None,
            primary_span,
            expansion_chain: Vec::new(),
        }
    }

    pub fn synthetic_at(span: SourceSpan) -> Self {
        Self::new(span)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub origin: SourceOrigin,
}

impl Expr {
    pub fn new(kind: ExprKind, origin: SourceOrigin) -> Self {
        Self { kind, origin }
    }

    pub fn synthetic(kind: ExprKind) -> Self {
        Self {
            kind,
            origin: SourceOrigin::synthetic_at(SourceSpan::new(0, 0)),
        }
    }

    pub fn synthetic_like(kind: ExprKind, source: &Expr) -> Self {
        Self {
            kind,
            origin: source.origin.clone(),
        }
    }

    pub fn with_origin_from(kind: ExprKind, source: &SourceOrigin) -> Self {
        Self {
            kind,
            origin: source.clone(),
        }
    }

    pub fn to_legacy(&self) -> Expression {
        match &self.kind {
            ExprKind::Symbol(value) => Expression::Symbol(value.clone()),
            ExprKind::Keyword(value) => Expression::Keyword(value.clone()),
            ExprKind::String(value) => Expression::String(value.clone()),
            ExprKind::QuoteSymbol(value) => Expression::QuoteSymbol(value.clone()),
            ExprKind::QuoteList(items) => {
                Expression::QuoteList(items.iter().map(Expr::to_legacy).collect())
            }
            ExprKind::Number(value) => Expression::Number(*value),
            ExprKind::List(items) => Expression::List(items.iter().map(Expr::to_legacy).collect()),
            ExprKind::Quasiquote(inner) => Expression::Quasiquote(Box::new(inner.to_legacy())),
            ExprKind::Unquote(inner) => Expression::Unquote(Box::new(inner.to_legacy())),
        }
    }

    pub fn from_legacy(expr: Expression) -> Self {
        let origin = SourceOrigin::synthetic_at(SourceSpan::new(0, 0));
        Self::from_legacy_with_origin(expr, &origin)
    }

    pub fn from_legacy_with_origin(expr: Expression, origin: &SourceOrigin) -> Self {
        let kind = match expr {
            Expression::Symbol(value) => ExprKind::Symbol(value),
            Expression::Keyword(value) => ExprKind::Keyword(value),
            Expression::String(value) => ExprKind::String(value),
            Expression::QuoteSymbol(value) => ExprKind::QuoteSymbol(value),
            Expression::QuoteList(items) => ExprKind::QuoteList(
                items
                    .into_iter()
                    .map(|item| Expr::from_legacy_with_origin(item, origin))
                    .collect(),
            ),
            Expression::Number(value) => ExprKind::Number(value),
            Expression::List(items) => ExprKind::List(
                items
                    .into_iter()
                    .map(|item| Expr::from_legacy_with_origin(item, origin))
                    .collect(),
            ),
            Expression::Quasiquote(inner) => {
                ExprKind::Quasiquote(Box::new(Expr::from_legacy_with_origin(*inner, origin)))
            }
            Expression::Unquote(inner) => {
                ExprKind::Unquote(Box::new(Expr::from_legacy_with_origin(*inner, origin)))
            }
        };
        Expr::with_origin_from(kind, origin)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Symbol(String),
    Keyword(String),
    String(String),
    QuoteSymbol(String),
    QuoteList(Vec<Expr>),
    Number(f64),
    List(Vec<Expr>),
    Quasiquote(Box<Expr>),
    Unquote(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParserError {
    ErrorParsingNumber,
    ExpectedLeftParen,
    ExpectedRightParen,
    ExpectedPipe,
    InvalidQuote,
    InvalidLambda,
    UnexpectedEOF,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    LeftParen,
    RightParen,
    Pipe,
    Symbol(String),
    Keyword(String), // :foo
    Number(f64),
    String(String),
    Quote,
    Backtick, // ` (quasiquote)
    Comma,    // , (unquote)
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpannedToken {
    pub token: Token,
    pub span: SourceSpan,
}

impl SpannedToken {
    fn new(token: Token, start_byte: usize, end_byte: usize) -> Self {
        Self {
            token,
            span: SourceSpan::new(start_byte, end_byte),
        }
    }
}

impl Parser {
    pub fn new(text: String) -> Self {
        #[cfg(test)]
        let input_bytes = text.len();
        Parser {
            text,
            pos: 0,
            #[cfg(test)]
            profile: std::cell::RefCell::new(ParserProfile {
                input_bytes,
                ..ParserProfile::default()
            }),
        }
    }

    fn peek(&self) -> Option<u8> {
        #[cfg(test)]
        {
            let mut profile = self.profile.borrow_mut();
            profile.peek_calls += 1;
            profile.estimated_char_visits += 1;
        }
        self.text.as_bytes().get(self.pos).copied()
    }

    fn next(&mut self) -> Option<u8> {
        if self.pos >= self.text.len() {
            return None;
        }
        #[cfg(test)]
        {
            let mut profile = self.profile.borrow_mut();
            profile.next_calls += 1;
            profile.estimated_char_visits += 1;
        }
        let next = self.text.as_bytes().get(self.pos).copied();
        self.pos += 1;
        next
    }

    fn peek_nth(&self, offset: usize) -> Option<u8> {
        #[cfg(test)]
        {
            let mut profile = self.profile.borrow_mut();
            profile.peek_nth_calls += 1;
            profile.estimated_char_visits += 1;
        }
        self.text.as_bytes().get(self.pos + offset).copied()
    }

    fn advance_char(&mut self) -> Option<char> {
        let rest = self.text.get(self.pos..)?;
        let ch = rest.chars().next()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }

    fn parse_text(
        &mut self,
        stop_at_whitespace: bool,
        stop_at_char: Option<u8>,
        is_numeric: bool,
    ) -> Result<String, ParserError> {
        #[cfg(test)]
        {
            self.profile.borrow_mut().parse_text_calls += 1;
        }
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if stop_at_whitespace && ch.is_ascii_whitespace() {
                break;
            }
            if let Some(stop) = stop_at_char
                && stop == ch
            {
                break;
            }
            if is_numeric && !ch.is_ascii_digit() {
                break;
            }
            if ch.is_ascii() {
                self.next();
            } else {
                self.advance_char();
            }
        }
        Ok(self.text[start..self.pos].to_string())
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek()
            && ch.is_ascii_whitespace()
        {
            #[cfg(test)]
            {
                self.profile.borrow_mut().skip_whitespace_loops += 1;
            }
            self.next();
        }
    }

    fn parse_symbol(&mut self) -> Result<Token, ParserError> {
        #[cfg(test)]
        {
            self.profile.borrow_mut().parse_symbol_calls += 1;
        }
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if ch.is_ascii_whitespace()
                || matches!(ch, b'(' | b')' | b'|' | b'\'' | b'"' | b'`' | b',')
            {
                break;
            }
            if ch.is_ascii() {
                self.next();
            } else {
                self.advance_char();
            }
        }
        Ok(Token::Symbol(self.text[start..self.pos].to_string()))
    }

    fn parse_number(&mut self) -> Result<Token, ParserError> {
        #[cfg(test)]
        {
            self.profile.borrow_mut().parse_number_calls += 1;
        }
        let start = self.pos;
        if matches!(self.peek(), Some(b'-')) {
            self.next();
        }
        let mut saw_digit = false;
        let mut saw_dot = false;
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                saw_digit = true;
                self.next();
            } else if ch == b'.' && !saw_dot {
                saw_dot = true;
                self.next();
            } else {
                break;
            }
        }
        if !saw_digit {
            return Err(ParserError::ErrorParsingNumber);
        }
        let text = &self.text[start..self.pos];
        Ok(Token::Number(
            text.parse().map_err(|_| ParserError::ErrorParsingNumber)?,
        ))
    }

    fn parse_string(&mut self) -> Result<Token, ParserError> {
        #[cfg(test)]
        {
            self.profile.borrow_mut().parse_string_calls += 1;
        }
        let text = self.parse_text(false, Some(b'"'), false)?;
        Ok(Token::String(text))
    }

    pub fn parse_spanned(&mut self) -> Result<Vec<SpannedToken>, ParserError> {
        let mut tokens: Vec<SpannedToken> = vec![];

        while self.peek().is_some() {
            self.skip_whitespace();
            if let Some(next) = self.peek() {
                let start = self.pos;
                match next {
                    b'(' => {
                        self.next();
                        tokens.push(SpannedToken::new(Token::LeftParen, start, self.pos));
                    }
                    b')' => {
                        self.next();
                        tokens.push(SpannedToken::new(Token::RightParen, start, self.pos));
                    }
                    b'|' => {
                        self.next();
                        tokens.push(SpannedToken::new(Token::Pipe, start, self.pos));
                    }
                    b'\'' => {
                        self.next();
                        tokens.push(SpannedToken::new(Token::Quote, start, self.pos));
                    }
                    b'`' => {
                        self.next();
                        tokens.push(SpannedToken::new(Token::Backtick, start, self.pos));
                    }
                    b',' => {
                        self.next();
                        tokens.push(SpannedToken::new(Token::Comma, start, self.pos));
                    }
                    b'"' => {
                        self.next();
                        let token = self.parse_string()?;
                        self.next();
                        tokens.push(SpannedToken::new(token, start, self.pos));
                    }
                    b';' => {
                        #[cfg(test)]
                        {
                            self.profile.borrow_mut().comments_skipped += 1;
                        }
                        // Skip comment to end of line
                        while matches!(self.peek(), Some(c) if c != b'\n') {
                            if self.peek().is_some_and(|c| c.is_ascii()) {
                                self.next();
                            } else {
                                self.advance_char();
                            }
                        }
                    }
                    b':' => {
                        self.next(); // consume ':'
                        let Token::Symbol(name) = self.parse_symbol()? else {
                            unreachable!()
                        };
                        tokens.push(SpannedToken::new(Token::Keyword(name), start, self.pos));
                    }
                    _ if next.is_ascii_digit()
                        || (next == b'-'
                            && matches!(self.peek_nth(1), Some(ch) if ch.is_ascii_digit()))
                        || (next == b'.'
                            && matches!(self.peek_nth(1), Some(ch) if ch.is_ascii_digit())) =>
                    {
                        let token = self.parse_number()?;
                        tokens.push(SpannedToken::new(token, start, self.pos));
                    }
                    _ if next.is_ascii_alphabetic() || next.is_ascii_punctuation() => {
                        let token = self.parse_symbol()?;
                        tokens.push(SpannedToken::new(token, start, self.pos));
                    }
                    _ => {
                        self.advance_char();
                    } // skip unknown chars (e.g. unicode outside strings)
                }
            }
        }
        #[cfg(test)]
        {
            self.profile.borrow_mut().tokens_emitted = tokens.len();
        }
        Ok(tokens)
    }

    pub fn parse(&mut self) -> Result<Vec<Token>, ParserError> {
        Ok(self
            .parse_spanned()?
            .into_iter()
            .map(|token| token.token)
            .collect())
    }

    #[cfg(test)]
    pub(crate) fn profile(&self) -> ParserProfile {
        self.profile.borrow().clone()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    Symbol(String),
    Keyword(String), // :foo
    String(String),
    QuoteSymbol(String),
    QuoteList(Vec<Expression>),
    Number(f64),
    List(Vec<Expression>),
    Quasiquote(Box<Expression>), // `expr
    Unquote(Box<Expression>),    // ,expr
}

pub struct ASTParser {
    tokens: Vec<Token>,
    pos: usize,
}

// choices to make: could create an AST first or just compile in one-shot
// lets make an AST first in parser

impl ASTParser {
    pub fn new(tokens: Vec<Token>) -> Self {
        ASTParser { tokens, pos: 0 }
    }

    pub fn peek(&self) -> Option<&Token> {
        if self.pos < self.tokens.len() {
            return self.tokens.get(self.pos);
        }
        None
    }

    pub fn next(&mut self) -> Option<&Token> {
        if self.pos < self.tokens.len() {
            let token = self.tokens.get(self.pos);
            self.pos += 1;
            return token;
        }
        None
    }

    pub fn parse_quote(&mut self) -> Result<Expression, ParserError> {
        match self.next() {
            Some(Token::Quote) => {}
            _ => return Err(ParserError::ExpectedLeftParen),
        }

        let next = self.peek();
        match next {
            None => Err(ParserError::UnexpectedEOF),
            Some(Token::Number(_)) => Err(ParserError::InvalidQuote),
            Some(Token::RightParen) => Err(ParserError::InvalidQuote),
            Some(Token::Pipe) => Err(ParserError::InvalidQuote),
            Some(Token::Quote) => Err(ParserError::InvalidQuote),
            Some(Token::String(_)) => Err(ParserError::InvalidQuote),
            Some(Token::Keyword(_)) => Err(ParserError::InvalidQuote),
            Some(Token::Backtick) => Err(ParserError::InvalidQuote),
            Some(Token::Comma) => Err(ParserError::InvalidQuote),
            Some(Token::Symbol(s)) => {
                let expression = Expression::QuoteSymbol(s.to_string());
                self.next();
                Ok(expression)
            }
            Some(Token::LeftParen) => {
                let list = self.parse_list()?;
                Ok(Expression::QuoteList(list))
            }
        }
    }

    fn parse_lambda_shorthand(&mut self) -> Result<Expression, ParserError> {
        match self.next() {
            Some(Token::Pipe) => {}
            _ => return Err(ParserError::ExpectedPipe),
        }

        let mut args = vec![];
        loop {
            match self.peek() {
                Some(Token::Pipe) => {
                    self.next();
                    break;
                }
                Some(Token::Symbol(s)) => {
                    args.push(Expression::Symbol(s.to_string()));
                    self.next();
                }
                Some(Token::LeftParen) => {
                    args.push(Expression::List(self.parse_list()?));
                }
                Some(_) => return Err(ParserError::InvalidLambda),
                None => return Err(ParserError::UnexpectedEOF),
            }
        }

        let body = self.parse_expression()?;
        Ok(Expression::List(vec![
            Expression::Symbol("lambda".to_string()),
            Expression::List(args),
            body,
        ]))
    }

    fn parse_expression(&mut self) -> Result<Expression, ParserError> {
        match self.peek() {
            Some(Token::LeftParen) => Ok(Expression::List(self.parse_list()?)),
            Some(Token::Quote) => self.parse_quote(),
            Some(Token::Number(n)) => {
                let value = *n;
                self.next();
                Ok(Expression::Number(value))
            }
            Some(Token::String(s)) => {
                let value = s.to_string();
                self.next();
                Ok(Expression::String(value))
            }
            Some(Token::Symbol(s)) => {
                let value = s.to_string();
                self.next();
                Ok(Expression::Symbol(value))
            }
            Some(Token::Keyword(k)) => {
                let value = k.to_string();
                self.next();
                Ok(Expression::Keyword(value))
            }
            Some(Token::Pipe) => self.parse_lambda_shorthand(),
            Some(Token::Backtick) => {
                self.next(); // consume backtick
                let expr = self.parse_expression()?;
                Ok(Expression::Quasiquote(Box::new(expr)))
            }
            Some(Token::Comma) => {
                self.next(); // consume comma
                let expr = self.parse_expression()?;
                Ok(Expression::Unquote(Box::new(expr)))
            }
            Some(Token::RightParen) => Err(ParserError::ExpectedLeftParen),
            None => Err(ParserError::UnexpectedEOF),
        }
    }

    pub fn parse_list(&mut self) -> Result<Vec<Expression>, ParserError> {
        match self.next() {
            Some(Token::LeftParen) => {}
            _ => return Err(ParserError::ExpectedLeftParen),
        }

        let mut list: Vec<Expression> = vec![];

        while let Some(token) = self.peek() {
            match token {
                Token::RightParen => {
                    self.next();
                    break;
                }
                _ => list.push(self.parse_expression()?),
            }
        }

        Ok(list)
    }

    pub fn parse(&mut self) -> Result<Vec<Expression>, ParserError> {
        let mut expressions = vec![];
        while self.peek().is_some() {
            expressions.push(self.parse_expression()?);
        }
        Ok(expressions)
    }
}

pub struct SpannedASTParser {
    tokens: Vec<SpannedToken>,
    pos: usize,
}

impl SpannedASTParser {
    pub fn new(tokens: Vec<SpannedToken>) -> Self {
        Self { tokens, pos: 0 }
    }

    pub fn peek(&self) -> Option<&SpannedToken> {
        self.tokens.get(self.pos)
    }

    pub fn next(&mut self) -> Option<SpannedToken> {
        if self.pos < self.tokens.len() {
            let token = self.tokens[self.pos].clone();
            self.pos += 1;
            Some(token)
        } else {
            None
        }
    }

    fn expr(&self, kind: ExprKind, span: SourceSpan) -> Expr {
        Expr::new(kind, SourceOrigin::new(span))
    }

    pub fn parse_quote(&mut self) -> Result<Expr, ParserError> {
        let quote = match self.next() {
            Some(token) if matches!(token.token, Token::Quote) => token,
            _ => return Err(ParserError::ExpectedLeftParen),
        };

        match self.peek().cloned() {
            None => Err(ParserError::UnexpectedEOF),
            Some(token)
                if matches!(
                    token.token,
                    Token::Number(_)
                        | Token::RightParen
                        | Token::Pipe
                        | Token::Quote
                        | Token::String(_)
                        | Token::Keyword(_)
                        | Token::Backtick
                        | Token::Comma
                ) =>
            {
                Err(ParserError::InvalidQuote)
            }
            Some(token) => match token.token {
                Token::Symbol(value) => {
                    let token = self.next().expect("peeked");
                    Ok(self.expr(
                        ExprKind::QuoteSymbol(value),
                        SourceSpan::new(quote.span.start_byte, token.span.end_byte),
                    ))
                }
                Token::LeftParen => {
                    let list = self.parse_list_expr()?;
                    let ExprKind::List(items) = list.kind else {
                        unreachable!();
                    };
                    Ok(self.expr(
                        ExprKind::QuoteList(items),
                        SourceSpan::new(quote.span.start_byte, list.origin.primary_span.end_byte),
                    ))
                }
                _ => Err(ParserError::InvalidQuote),
            },
        }
    }

    fn parse_lambda_shorthand(&mut self) -> Result<Expr, ParserError> {
        let start = match self.next() {
            Some(token) if matches!(token.token, Token::Pipe) => token.span.start_byte,
            _ => return Err(ParserError::ExpectedPipe),
        };

        let mut args = Vec::new();
        let pipe_end;
        loop {
            match self.peek().cloned() {
                Some(token) if matches!(token.token, Token::Pipe) => {
                    pipe_end = token.span.end_byte;
                    self.next();
                    break;
                }
                Some(SpannedToken {
                    token: Token::Symbol(value),
                    span,
                }) => {
                    self.next();
                    args.push(self.expr(ExprKind::Symbol(value), span));
                }
                Some(token) if matches!(token.token, Token::LeftParen) => {
                    args.push(self.parse_list_expr()?);
                }
                Some(_) => return Err(ParserError::InvalidLambda),
                None => return Err(ParserError::UnexpectedEOF),
            }
        }

        let body = self.parse_expression()?;
        let full_span = SourceSpan::new(start, body.origin.primary_span.end_byte);
        let origin = SourceOrigin::new(full_span.clone());
        let lambda_symbol = Expr::with_origin_from(ExprKind::Symbol("lambda".to_string()), &origin);
        let arg_list = Expr::with_origin_from(ExprKind::List(args), &origin);
        let end = body.origin.primary_span.end_byte.max(pipe_end);
        Ok(self.expr(
            ExprKind::List(vec![lambda_symbol, arg_list, body]),
            SourceSpan::new(start, end),
        ))
    }

    fn parse_expression(&mut self) -> Result<Expr, ParserError> {
        let Some(token) = self.peek().cloned() else {
            return Err(ParserError::UnexpectedEOF);
        };
        match token.token {
            Token::LeftParen => self.parse_list_expr(),
            Token::Quote => self.parse_quote(),
            Token::Number(value) => {
                self.next();
                Ok(self.expr(ExprKind::Number(value), token.span))
            }
            Token::String(value) => {
                self.next();
                Ok(self.expr(ExprKind::String(value), token.span))
            }
            Token::Symbol(value) => {
                self.next();
                Ok(self.expr(ExprKind::Symbol(value), token.span))
            }
            Token::Keyword(value) => {
                self.next();
                Ok(self.expr(ExprKind::Keyword(value), token.span))
            }
            Token::Pipe => self.parse_lambda_shorthand(),
            Token::Backtick => {
                self.next();
                let expr = self.parse_expression()?;
                Ok(self.expr(
                    ExprKind::Quasiquote(Box::new(expr.clone())),
                    SourceSpan::new(token.span.start_byte, expr.origin.primary_span.end_byte),
                ))
            }
            Token::Comma => {
                self.next();
                let expr = self.parse_expression()?;
                Ok(self.expr(
                    ExprKind::Unquote(Box::new(expr.clone())),
                    SourceSpan::new(token.span.start_byte, expr.origin.primary_span.end_byte),
                ))
            }
            Token::RightParen => Err(ParserError::ExpectedLeftParen),
        }
    }

    pub fn parse_list_expr(&mut self) -> Result<Expr, ParserError> {
        let start = match self.next() {
            Some(token) if matches!(token.token, Token::LeftParen) => token.span.start_byte,
            _ => return Err(ParserError::ExpectedLeftParen),
        };

        let mut items = Vec::new();
        while let Some(token) = self.peek().cloned() {
            if matches!(token.token, Token::RightParen) {
                self.next();
                return Ok(self.expr(
                    ExprKind::List(items),
                    SourceSpan::new(start, token.span.end_byte),
                ));
            }
            items.push(self.parse_expression()?);
        }

        Err(ParserError::UnexpectedEOF)
    }

    pub fn parse(&mut self) -> Result<Vec<Expr>, ParserError> {
        let mut expressions = Vec::new();
        while self.peek().is_some() {
            expressions.push(self.parse_expression()?);
        }
        Ok(expressions)
    }
}

/// Format an Expression back to a Lisp source string.
pub fn format_expression(expr: &Expression) -> String {
    match expr {
        Expression::Number(n) => {
            if *n == n.trunc() && n.abs() < 1e15 {
                format!("{:.1}", n)
            } else {
                format!("{}", n)
            }
        }
        Expression::Symbol(s) => s.clone(),
        Expression::Keyword(s) => format!(":{}", s),
        Expression::String(s) => format!("\"{}\"", s),
        Expression::QuoteSymbol(s) => format!("'{}", s),
        Expression::QuoteList(items) => {
            let inner: Vec<String> = items.iter().map(format_expression).collect();
            format!("'({})", inner.join(" "))
        }
        Expression::List(items) => {
            let inner: Vec<String> = items.iter().map(format_expression).collect();
            format!("({})", inner.join(" "))
        }
        Expression::Quasiquote(inner) => format!("`{}", format_expression(inner)),
        Expression::Unquote(inner) => format!(",{}", format_expression(inner)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(input: &str) -> Vec<Expression> {
        let tokens = Parser::new(input.to_string()).parse().unwrap();
        let mut ast = ASTParser::new(tokens);
        ast.parse().unwrap()
    }

    fn parse_spanned_str(input: &str) -> Vec<Expr> {
        let tokens = Parser::new(input.to_string()).parse_spanned().unwrap();
        SpannedASTParser::new(tokens).parse().unwrap()
    }

    #[test]
    fn negative_number_literal() {
        let exprs = parse_str("(foo :min -10 :max 10)");
        let Expression::List(list) = &exprs[0] else {
            panic!("expected list");
        };
        assert!(matches!(&list[0], Expression::Symbol(s) if s == "foo"));
        assert!(matches!(&list[1], Expression::Keyword(k) if k == "min"));
        assert!(matches!(&list[2], Expression::Number(n) if *n == -10.0));
        assert!(matches!(&list[3], Expression::Keyword(k) if k == "max"));
        assert!(matches!(&list[4], Expression::Number(n) if *n == 10.0));
    }

    #[test]
    fn negative_float_literal() {
        let exprs = parse_str("-3.14");
        assert!(matches!(&exprs[0], Expression::Number(n) if (*n - -3.14).abs() < 0.001));
    }

    #[test]
    fn bare_minus_is_symbol() {
        let exprs = parse_str("(- 1 2)");
        let Expression::List(list) = &exprs[0] else {
            panic!("expected list");
        };
        assert!(matches!(&list[0], Expression::Symbol(s) if s == "-"));
    }

    #[test]
    fn spanned_parser_tracks_lists_strings_comments_and_unicode() {
        let source = "; leading comment\n(box :text \"hé\")";
        let exprs = parse_spanned_str(source);
        assert_eq!(exprs.len(), 1);
        let form_start = source.find("(box").unwrap();
        assert_eq!(
            exprs[0].origin.primary_span,
            SourceSpan::new(form_start, source.len())
        );
        let ExprKind::List(items) = &exprs[0].kind else {
            panic!("expected list");
        };
        assert_eq!(
            items[0].origin.primary_span,
            SourceSpan::new(form_start + 1, form_start + 4)
        );
        let string_start = source.find("\"hé\"").unwrap();
        assert_eq!(
            items[2].origin.primary_span,
            SourceSpan::new(string_start, string_start + "\"hé\"".len())
        );
    }

    #[test]
    fn spanned_parser_tracks_lambda_shorthand_body_span() {
        let source = "(each xs |x| (label x))";
        let exprs = parse_spanned_str(source);
        let ExprKind::List(items) = &exprs[0].kind else {
            panic!("expected each list");
        };
        let lambda = &items[2];
        let lambda_start = source.find("|x|").unwrap();
        let lambda_end = source.rfind("))").unwrap() + 1;
        assert_eq!(
            lambda.origin.primary_span,
            SourceSpan::new(lambda_start, lambda_end)
        );
        let ExprKind::List(lambda_items) = &lambda.kind else {
            panic!("expected lambda desugaring");
        };
        assert!(matches!(&lambda_items[0].kind, ExprKind::Symbol(symbol) if symbol == "lambda"));
        assert_eq!(
            lambda_items[2].origin.primary_span,
            SourceSpan::new(source.find("(label").unwrap(), lambda_end)
        );
    }

    #[test]
    fn spanned_parser_tracks_quote_quasiquote_and_unquote_spans() {
        let source = "'(a b) `(box ,x)";
        let exprs = parse_spanned_str(source);
        assert_eq!(exprs.len(), 2);
        assert_eq!(exprs[0].origin.primary_span, SourceSpan::new(0, 6));
        assert!(matches!(exprs[0].kind, ExprKind::QuoteList(_)));
        let quasi_start = source.find('`').unwrap();
        assert_eq!(
            exprs[1].origin.primary_span,
            SourceSpan::new(quasi_start, source.len())
        );
        let ExprKind::Quasiquote(inner) = &exprs[1].kind else {
            panic!("expected quasiquote");
        };
        let ExprKind::List(items) = &inner.kind else {
            panic!("expected quasiquoted list");
        };
        let unquote_start = source.find(",x").unwrap();
        assert_eq!(
            items[1].origin.primary_span,
            SourceSpan::new(unquote_start, unquote_start + 2)
        );
        assert!(matches!(items[1].kind, ExprKind::Unquote(_)));
    }

    #[test]
    fn spanned_parser_reports_unclosed_list_as_unexpected_eof() {
        let tokens = Parser::new("(box :text \"x\"".to_string())
            .parse_spanned()
            .unwrap();
        let result = SpannedASTParser::new(tokens).parse();
        assert_eq!(result, Err(ParserError::UnexpectedEOF));
    }
}
