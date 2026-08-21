use std::path::PathBuf;

use eseqlisp::module_export_migration::{apply_migration, plan_migration, unified_diff};

fn usage() -> &'static str {
    "Usage: eseqlisp_migrate_module_exports (--dry-run | --write) <module-or-family> -- <file-or-directory>...\n\
     \n\
     Converts one exact module or dotted module family. The supplied paths are\n\
     both the module discovery roots and the tree-wide qualified-reference scan.\n\
     Comments are left unchanged; possible references in strings are reported\n\
     for manual review. No file is changed unless --write is explicit."
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<i32, String> {
    let mut write = None;
    let mut selector = None;
    let mut paths = Vec::new();
    let mut positional_paths = false;
    for argument in std::env::args().skip(1) {
        if positional_paths {
            paths.push(PathBuf::from(argument));
            continue;
        }
        match argument.as_str() {
            "--dry-run" if write.is_none() => write = Some(false),
            "--write" if write.is_none() => write = Some(true),
            "--dry-run" | "--write" => {
                return Err("choose exactly one of --dry-run or --write".to_string());
            }
            "--" => positional_paths = true,
            "-h" | "--help" => {
                println!("{}", usage());
                return Ok(0);
            }
            _ if argument.starts_with('-') => {
                return Err(format!("unknown option: {argument}\n{}", usage()));
            }
            _ if selector.is_none() => selector = Some(argument),
            _ => paths.push(PathBuf::from(argument)),
        }
    }
    let write = write.ok_or_else(|| format!("migration mode is required\n{}", usage()))?;
    let selector = selector.ok_or_else(|| format!("module selector is required\n{}", usage()))?;
    if paths.is_empty() {
        return Err(format!("at least one scan path is required\n{}", usage()));
    }

    let plan = plan_migration(&selector, &paths).map_err(|error| error.to_string())?;
    for file in &plan.files {
        for occurrence in &file.manual {
            eprintln!(
                "{}:{}:{}: manual string: `{}` -> `{}`",
                file.path.display(),
                occurrence.line,
                occurrence.column,
                occurrence.old,
                occurrence.new,
            );
        }
    }

    if write {
        apply_migration(&plan).map_err(|error| error.to_string())?;
        println!(
            "migrated {}: {} edits in {} files; {} manual string occurrences left unchanged",
            plan.modules.join(", "),
            plan.replacement_count(),
            plan.changed_file_count(),
            plan.manual_count(),
        );
    } else {
        for file in plan.changed_files() {
            print!("{}", unified_diff(file));
        }
        println!(
            "dry run for {}: {} edits in {} files; no files changed; {} manual string occurrences left unchanged",
            plan.modules.join(", "),
            plan.replacement_count(),
            plan.changed_file_count(),
            plan.manual_count(),
        );
    }
    Ok(if plan.manual_count() == 0 { 0 } else { 1 })
}
