pub struct Parser {
    text: String,
    pos: usize,
}

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
}

impl Parser {
    pub fn new(text: String) -> Self {
        Parser { text, pos: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.text.chars().nth(self.pos)
    }

    fn next(&mut self) -> Option<char> {
        if self.pos >= self.text.len() {
            return None;
        }
        let next = self.text.chars().nth(self.pos);
        self.pos += 1;
        next
    }

    fn peek_nth(&self, offset: usize) -> Option<char> {
        self.text.chars().nth(self.pos + offset)
    }

    fn parse_text(
        &mut self,
        stop_at_whitespace: bool,
        stop_at_char: Option<char>,
        is_numeric: bool,
    ) -> Result<String, ParserError> {
        let mut str = String::new();
        while let Some(ch) = self.peek() {
            if stop_at_whitespace && ch.is_whitespace() {
                break;
            }
            if let Some(stop) = stop_at_char
                && stop == ch
            {
                break;
            }
            if is_numeric && !ch.is_numeric() {
                break;
            }
            str.push(ch);
            self.next();
        }
        Ok(str)
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek()
            && ch.is_whitespace()
        {
            self.next();
        }
    }

    fn parse_symbol(&mut self) -> Result<Token, ParserError> {
        let mut text = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() || matches!(ch, '(' | ')' | '|' | '\'' | '\"') {
                break;
            }
            text.push(ch);
            self.next();
        }
        Ok(Token::Symbol(text))
    }

    fn parse_number(&mut self) -> Result<Token, ParserError> {
        let mut text = String::new();
        if matches!(self.peek(), Some('-')) {
            text.push('-');
            self.next();
        }
        let mut saw_digit = false;
        let mut saw_dot = false;
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                saw_digit = true;
                text.push(ch);
                self.next();
            } else if ch == '.' && !saw_dot {
                saw_dot = true;
                text.push(ch);
                self.next();
            } else {
                break;
            }
        }
        if !saw_digit {
            return Err(ParserError::ErrorParsingNumber);
        }
        Ok(Token::Number(
            text.parse().map_err(|_| ParserError::ErrorParsingNumber)?,
        ))
    }

    fn parse_string(&mut self) -> Result<Token, ParserError> {
        let text = self.parse_text(false, Some('\"'), false)?;
        Ok(Token::String(text))
    }

    pub fn parse(&mut self) -> Result<Vec<Token>, ParserError> {
        let mut tokens: Vec<Token> = vec![];

        while self.peek().is_some() {
            self.skip_whitespace();
            if let Some(next) = self.peek() {
                match next {
                    '(' => {
                        tokens.push(Token::LeftParen);
                        self.next();
                    }
                    ')' => {
                        tokens.push(Token::RightParen);
                        self.next();
                    }
                    '|' => {
                        tokens.push(Token::Pipe);
                        self.next();
                    }
                    '\'' => {
                        tokens.push(Token::Quote);
                        self.next();
                    }
                    '\"' => {
                        self.next();
                        tokens.push(self.parse_string()?);
                        self.next();
                    }
                    ';' => {
                        // Skip comment to end of line
                        while matches!(self.peek(), Some(c) if c != '\n') {
                            self.next();
                        }
                    }
                    ':' => {
                        self.next(); // consume ':'
                        let Token::Symbol(name) = self.parse_symbol()? else {
                            unreachable!()
                        };
                        tokens.push(Token::Keyword(name));
                    }
                    _ if next.is_alphabetic() || next.is_ascii_punctuation() => {
                        tokens.push(self.parse_symbol()?);
                    }
                    _ if next.is_ascii_digit()
                        || (next == '-'
                            && matches!(self.peek_nth(1), Some(ch) if ch.is_ascii_digit()))
                        || (next == '.'
                            && matches!(self.peek_nth(1), Some(ch) if ch.is_ascii_digit())) =>
                    {
                        tokens.push(self.parse_number()?);
                    }
                    _ => {
                        self.next();
                    } // skip unknown chars (e.g. unicode outside strings)
                }
            }
        }
        Ok(tokens)
    }
}

#[derive(Debug, Clone)]
pub enum Expression {
    Symbol(String),
    Keyword(String), // :foo
    String(String),
    QuoteSymbol(String),
    QuoteList(Vec<Expression>),
    Number(f64),
    List(Vec<Expression>),
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
