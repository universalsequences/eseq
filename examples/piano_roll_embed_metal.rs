mod piano_roll_embed_common;

#[cfg(target_os = "macos")]
fn main() {
    piano_roll_embed_common::run_metal()
        .unwrap_or_else(|_| eprintln!("Metal backend error"));
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("piano_roll_embed_metal is only available on macOS");
}
