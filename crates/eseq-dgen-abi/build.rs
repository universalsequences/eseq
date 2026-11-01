// The DGen ABI v1 host-services tables (`DGenHostServicesV1`) generated
// dylibs call through: Accelerate/vDSP on Apple, the portable C kernel
// everywhere (sequencer's dgen_fft.rs falls back to it off Apple).
fn main() {
    let audiograph = "../sequencer/audiograph";
    let mut build = cc::Build::new();
    build
        .file(format!("{audiograph}/dgen_host_services.c"))
        .file(format!("{audiograph}/dgen_fft.c"))
        .include(audiograph)
        .flag("-std=c11")
        .flag("-O2");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        build.define("_GNU_SOURCE", None);
    }
    build.compile("eseq_dgen_host_services");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=Accelerate");
    }
    for file in ["dgen_host_services.c", "dgen_host_services.h", "dgen_fft.c", "dgen_fft.h", "dgen_abi_v1.h"] {
        println!("cargo:rerun-if-changed={audiograph}/{file}");
    }
    println!("cargo:rerun-if-env-changed=CFLAGS");
    println!("cargo:rerun-if-env-changed=CC");
}
