fn main() {
    emit_dev_manifest_dir();

    let mut audiograph = cc::Build::new();
    audiograph
        .files([
            "audiograph/graph_api.c",
            "audiograph/graph_edit.c",
            "audiograph/graph_engine.c",
            "audiograph/graph_nodes.c",
            "audiograph/hot_swap.c",
            "audiograph/ready_queue.c",
            "audiograph/wrapper.c",
            "audiograph/dgen_host_services.c",
            // Compiled on every target, not just the ones whose host-services
            // table uses it: the tests that pin the portable FFT to the same
            // reference vDSP is pinned to have to run on macOS too, or the two
            // backends were never actually compared (eseq-linux.9).
            "audiograph/dgen_fft.c",
        ])
        .include("audiograph")
        .flag("-std=c11")
        .flag("-O2")
        .flag("-pthread");

    // Strict C11 suppresses glibc's default feature set. Audiograph uses GNU/
    // POSIX APIs including pthread rwlocks, posix_memalign, strdup, and usleep.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        audiograph.define("_GNU_SOURCE", None);
    }

    audiograph.compile("audiograph");

    // On Apple platforms the DGen ABI v1 host-services table stays vDSP-backed,
    // so the app links Accelerate (generated DGen dylibs never do — they stay
    // libSystem-only). Elsewhere the table is served by audiograph/dgen_fft.c
    // and there is nothing to link (eseq-linux.9).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=Accelerate");
    }

    println!("cargo:rerun-if-changed=audiograph/");
    println!("cargo:rerun-if-env-changed=CFLAGS");
    println!("cargo:rerun-if-env-changed=CC");
}

/// The crate's own checkout path, or a synthetic placeholder when
/// `ESEQ_PACKAGED` is set. `env!("CARGO_MANIFEST_DIR")` bakes an absolute path
/// into the build machine's checkout, and every remaining use of it in shipped
/// code is a dev-only fallback already guarded by an existence check, so a
/// packaged binary wants those literals gone -- see
/// `docs/release-packaging-spec.md` sections 4.1 and 5. This is provenance
/// hygiene only; the Dev/Release split itself stays a runtime decision.
fn emit_dev_manifest_dir() {
    println!("cargo:rerun-if-env-changed=ESEQ_PACKAGED");
    let dir = if std::env::var_os("ESEQ_PACKAGED").is_some() {
        format!(
            "/eseq-packaged/crates/{}",
            std::env::var("CARGO_PKG_NAME").unwrap()
        )
    } else {
        std::env::var("CARGO_MANIFEST_DIR").unwrap()
    };
    println!("cargo:rustc-env=ESEQ_DEV_MANIFEST_DIR={dir}");
}
