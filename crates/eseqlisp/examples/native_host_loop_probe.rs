#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    eseqlisp::metal_backend::run_native_host_probe()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("native_host_loop_probe requires macOS");
}
