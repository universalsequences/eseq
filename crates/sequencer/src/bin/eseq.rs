use std::path::PathBuf;

fn main() {
    if let Err(error) = run(std::env::args().skip(1).collect()) {
        eprintln!("eseq: {error}");
        std::process::exit(1);
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    match args.as_slice() {
        [package, index] if package == "package" && index == "index" => {
            index_package(std::env::current_dir().map_err(|error| error.to_string())?)
        }
        [package, index, path] if package == "package" && index == "index" => {
            index_package(PathBuf::from(path))
        }
        _ => Err("usage: eseq package index [PACKAGE_DIR]".to_string()),
    }
}

fn index_package(path: PathBuf) -> Result<(), String> {
    let lines = sequencer::sample_manifest::index_package(&path)?;
    let count = lines
        .iter()
        .filter(|line| {
            matches!(
                line,
                sequencer::sample_manifest::SampleManifestLine::Sample(_)
            )
        })
        .count();
    println!(
        "indexed {count} sample(s) into {}",
        path.join("samples.jsonl").display()
    );
    Ok(())
}
