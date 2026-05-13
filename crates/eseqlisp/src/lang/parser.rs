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

#[derive(Debug)]
pub enum ParserError {
    ErrorParsingNumber,
    ExpectedLeftParen,
    ExpectedRightParen,
    ExpectedPipe,
    InvalidQuote,
    InvalidLambda,
    UnexpectedEOF,
}

#[derive(Debug)]
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

    pub fn parse(&mut self) -> Result<Vec<Token>, ParserError> {
        let mut tokens: Vec<Token> = vec![];

        while self.peek().is_some() {
            self.skip_whitespace();
            if let Some(next) = self.peek() {
                match next {
                    b'(' => {
                        tokens.push(Token::LeftParen);
                        self.next();
                    }
                    b')' => {
                        tokens.push(Token::RightParen);
                        self.next();
                    }
                    b'|' => {
                        tokens.push(Token::Pipe);
                        self.next();
                    }
                    b'\'' => {
                        tokens.push(Token::Quote);
                        self.next();
                    }
                    b'`' => {
                        tokens.push(Token::Backtick);
                        self.next();
                    }
                    b',' => {
                        tokens.push(Token::Comma);
                        self.next();
                    }
                    b'"' => {
                        self.next();
                        tokens.push(self.parse_string()?);
                        self.next();
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
                        tokens.push(Token::Keyword(name));
                    }
                    _ if next.is_ascii_digit()
                        || (next == b'-'
                            && matches!(self.peek_nth(1), Some(ch) if ch.is_ascii_digit()))
                        || (next == b'.'
                            && matches!(self.peek_nth(1), Some(ch) if ch.is_ascii_digit())) =>
                    {
                        tokens.push(self.parse_number()?);
                    }
                    _ if next.is_ascii_alphabetic() || next.is_ascii_punctuation() => {
                        tokens.push(self.parse_symbol()?);
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
}
