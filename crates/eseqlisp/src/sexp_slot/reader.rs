//! Typed text -> data (docs/sexp-slot-spec.md §8.1). Exactly one
//! s-expression, parsed by the language's own tokenizer and parser and turned
//! into plain values. Nothing is ever evaluated: quotes, quasiquotes, unquotes
//! and lambda shorthand are rejected, so a slot can only ever hold data.

use std::cell::RefCell;
use std::rc::Rc;

use crate::parser::{ASTParser, Expression, Parser, ParserError, Token};
use crate::vm::Value;

/// Parse `text` as exactly one s-expression of data. Missing trailing `)`
/// close at the end of the text (the language's tokenizer does this), so a
/// quickly typed `(1 2 3` reads as `(1 2 3)`. Numbers, strings and
/// keywords read as themselves, `true` / `false` / `nil` as their values,
/// any other symbol as a symbol, and lists nest. The error is a short
/// reason fit to show under a text field.
pub fn read_value(text: &str) -> Result<Value, String> {
    let tokens = Parser::new(text.to_string())
        .parse()
        .map_err(|error| parser_reason(&error))?;
    // `|x| body` desugars to a lambda list; a slot never holds code.
    if tokens.iter().any(|token| matches!(token, Token::Pipe)) {
        return Err("| is not allowed here".to_string());
    }
    let expressions = ASTParser::new(tokens)
        .parse()
        .map_err(|error| parser_reason(&error))?;
    match expressions.as_slice() {
        [] => Err("nothing to read".to_string()),
        [expression] => expression_value(expression),
        _ => Err("expected one value; wrap several in ( )".to_string()),
    }
}

fn parser_reason(error: &ParserError) -> String {
    match error {
        ParserError::ErrorParsingNumber => "not a number".to_string(),
        ParserError::ExpectedRightParen | ParserError::UnexpectedEOF => {
            "missing )".to_string()
        }
        ParserError::ExpectedLeftParen => "unexpected )".to_string(),
        ParserError::ExpectedPipe | ParserError::InvalidLambda => {
            "| is not allowed here".to_string()
        }
        ParserError::InvalidQuote => "quotes are not allowed here".to_string(),
    }
}

fn expression_value(expression: &Expression) -> Result<Value, String> {
    Ok(match expression {
        Expression::Number(number) => Value::Number(*number),
        Expression::String(text) => Value::String(text.clone()),
        Expression::Keyword(name) => Value::Keyword(name.clone()),
        Expression::Symbol(name) => match name.as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            "nil" => Value::Nil,
            _ => Value::Symbol(name.clone()),
        },
        Expression::List(items) => Value::List(
            items
                .iter()
                .map(|item| expression_value(item).map(|value| Rc::new(RefCell::new(value))))
                .collect::<Result<_, _>>()?,
        ),
        Expression::QuoteSymbol(_)
        | Expression::QuoteList(_)
        | Expression::Quasiquote(_)
        | Expression::Unquote(_)
        | Expression::UnquoteSplicing(_) => {
            return Err("quotes are not allowed here".to_string());
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: Vec<Value>) -> Value {
        Value::List(items.into_iter().map(|v| Rc::new(RefCell::new(v))).collect())
    }

    #[test]
    fn reads_atoms_and_nested_lists_as_data() {
        assert_eq!(read_value("3"), Ok(Value::Number(3.0)));
        assert_eq!(read_value(" -1.5 "), Ok(Value::Number(-1.5)));
        assert_eq!(read_value("rev"), Ok(Value::Symbol("rev".into())));
        assert_eq!(read_value(":16t"), Ok(Value::Keyword("16t".into())));
        assert_eq!(
            read_value("(1 2 (3 4))"),
            Ok(list(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                list(vec![Value::Number(3.0), Value::Number(4.0)]),
            ]))
        );
        assert_eq!(
            read_value("(every 2 (rev swap))"),
            Ok(list(vec![
                Value::Symbol("every".into()),
                Value::Number(2.0),
                list(vec![Value::Symbol("rev".into()), Value::Symbol("swap".into())]),
            ]))
        );
    }

    #[test]
    fn rejects_anything_that_is_not_one_piece_of_data() {
        assert_eq!(read_value(""), Err("nothing to read".into()));
        assert_eq!(read_value("1 2"), Err("expected one value; wrap several in ( )".into()));
        // Missing trailing parens close at the end, so `(1 2 3 4` + Enter
        // is a fast way to type a list.
        assert_eq!(
            read_value("(1 (2 3"),
            Ok(list(vec![
                Value::Number(1.0),
                list(vec![Value::Number(2.0), Value::Number(3.0)]),
            ]))
        );
        assert!(read_value("1)").is_err());
        assert!(read_value("'(1 2)").is_err());
        assert!(read_value("`(1 ,x)").is_err());
        assert!(read_value("|x| x").is_err());
    }
}
