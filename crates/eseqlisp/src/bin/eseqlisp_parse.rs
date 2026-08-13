use std::path::PathBuf;

use eseqlisp::parser::{ASTParser, Parser};

fn main() {
    let paths = std::env::args_os().skip(1).map(PathBuf::from).collect::<Vec<_>>();
    if paths.is_empty() {
        eprintln!("usage: eseqlisp_parse <file.lisp>...");
        std::process::exit(2);
    }

    let mut failures = 0;
    for path in &paths {
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("{}: read error: {error}", path.display());
                failures += 1;
                continue;
            }
        };
        let tokens = match Parser::new(source).parse() {
            Ok(tokens) => tokens,
            Err(error) => {
                eprintln!("{}: reader error: {error:?}", path.display());
                failures += 1;
                continue;
            }
        };
        if let Err(error) = ASTParser::new(tokens).parse() {
            eprintln!("{}: AST error: {error:?}", path.display());
            failures += 1;
        }
    }

    if failures != 0 {
        eprintln!("{failures} of {} Lisp files failed to parse", paths.len());
        std::process::exit(1);
    }
    println!("parsed {} Lisp files", paths.len());
}
