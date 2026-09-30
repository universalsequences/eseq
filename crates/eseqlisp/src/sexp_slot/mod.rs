//! sexp slots: Lisp on guard rails (docs/sexp-slot-spec.md).
//!
//! A slot edits one small, guarded piece of Lisp data: a number, a word, a
//! list of those (nesting allowed) or a form (a head word plus guarded args).
//! This module holds the data side the widget and Lisp share: the reader
//! (typed text -> data, never evaluated) and the schema model (the guard
//! rails: validate, clamp, default, complete).

pub mod edit;
mod reader;
pub mod schema;

pub use reader::read_value;
