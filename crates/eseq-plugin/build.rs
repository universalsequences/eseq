// The DGen FFT host services (`DGenHostServicesV1`) spectral instruments call
// through. Same C the sequencer links; Accelerate-backed on macOS.
fn main() {
    let audiograph = "../sequencer/audiograph";
    println!("cargo:rerun-if-changed={audiograph}/dgen_host_services.c");
    println!("cargo:rerun-if-changed={audiograph}/dgen_fft.c");
    cc::Build::new()
        .file(format!("{audiograph}/dgen_host_services.c"))
        .file(format!("{audiograph}/dgen_fft.c"))
        .include(audiograph)
        .opt_level(3)
        .compile("eseq_plugin_dgen_host_services");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=Accelerate");
    }
}
