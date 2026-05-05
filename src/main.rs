#![allow(clippy::needless_return)]

fn main() {
    let metal = std::env::args().any(|a| a == "--metal");

    if metal {
        #[cfg(target_os = "macos")]
        {
            eseqlisp::run_metal().unwrap_or_else(|_| eprintln!("Metal backend error"));
            return;
        }
        #[cfg(not(target_os = "macos"))]
        eprintln!("Metal is macOS only");
    } else {
        eseqlisp::run_standalone().expect("ratatui error");
    }
}
