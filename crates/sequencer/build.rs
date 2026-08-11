fn main() {
    cc::Build::new()
        .files([
            "audiograph/graph_api.c",
            "audiograph/graph_edit.c",
            "audiograph/graph_engine.c",
            "audiograph/graph_nodes.c",
            "audiograph/hot_swap.c",
            "audiograph/ready_queue.c",
            "audiograph/wrapper.c",
            "audiograph/dgen_host_services.c",
        ])
        .include("audiograph")
        .flag("-std=c11")
        .flag("-O2")
        .flag("-pthread")
        .compile("audiograph");

    // The DGen ABI v1 host-services table is vDSP-backed; the app links
    // Accelerate (generated DGen dylibs never do — they stay libSystem-only).
    println!("cargo:rustc-link-lib=framework=Accelerate");

    println!("cargo:rerun-if-changed=audiograph/");
    println!("cargo:rerun-if-env-changed=CFLAGS");
    println!("cargo:rerun-if-env-changed=CC");
}
