fn main() {
    emit_dev_manifest_dir();
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
