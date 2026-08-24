#![allow(
    dead_code,
    clippy::inspect_for_each,
    clippy::manual_clamp,
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::useless_conversion,
    clippy::useless_format
)]

#[cfg(target_os = "macos")]
include!("metal_main.rs");

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("metal_seq's Metal UI is macOS only.");
}
