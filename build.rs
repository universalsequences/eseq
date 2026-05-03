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
        ])
        .include("audiograph")
        .flag("-std=c11")
        .flag("-O2")
        .flag("-pthread")
        .compile("audiograph");

    println!("cargo:rerun-if-changed=audiograph/");
    println!("cargo:rerun-if-env-changed=CFLAGS");
    println!("cargo:rerun-if-env-changed=CC");
}
