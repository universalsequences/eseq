use std::path::PathBuf;

use eseqlisp::module_alias_migration::{apply_migration, plan_migration, unified_diff};

fn usage() -> &'static str {
    "Usage: eseqlisp_migrate_module_aliases (--dry-run | --write) -- <file-or-directory>...\n\
     \n\
     Rewrites only executable symbols and comments. Strings and quoted data are\n\
     reported for manual review. No file is changed unless --write is explicit."
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
    let mut paths = Vec::new();
    let mut positional = false;
    for argument in std::env::args().skip(1) {
        if positional {
            paths.push(PathBuf::from(argument));
            continue;
        }
        match argument.as_str() {
            "--dry-run" if write.is_none() => write = Some(false),
            "--write" if write.is_none() => write = Some(true),
            "--dry-run" | "--write" => {
                return Err("choose exactly one of --dry-run or --write".to_string());
            }
            "--" => positional = true,
            "-h" | "--help" => {
                println!("{}", usage());
                return Ok(0);
            }
            _ if argument.starts_with('-') => {
                return Err(format!("unknown option: {argument}\n{}", usage()));
            }
            _ => paths.push(PathBuf::from(argument)),
        }
    }
    let write = write.ok_or_else(|| format!("migration mode is required\n{}", usage()))?;
    if paths.is_empty() {
        return Err(format!(
            "at least one file or directory is required\n{}",
            usage()
        ));
    }

    let plan = plan_migration(&paths).map_err(|error| error.to_string())?;
    for file in &plan.files {
        for occurrence in &file.manual {
            eprintln!(
                "{}:{}:{}: manual {}: `{}` -> `{}`",
                file.path.display(),
                occurrence.line,
                occurrence.column,
                match occurrence.kind {
                    eseqlisp::module_alias_migration::OccurrenceKind::String => "string",
                    eseqlisp::module_alias_migration::OccurrenceKind::Quoted => "quoted data",
                    _ => unreachable!("manual occurrences are string or quoted data"),
                },
                occurrence.old,
                occurrence.new,
            );
        }
    }

    if write {
        apply_migration(&plan).map_err(|error| error.to_string())?;
        println!(
            "migrated: {} replacements in {} files; {} manual occurrences left unchanged",
            plan.replacement_count(),
            plan.changed_file_count(),
            plan.manual_count(),
        );
    } else {
        for file in plan.changed_files() {
            print!("{}", unified_diff(file));
        }
        println!(
            "dry run: {} replacements in {} files; no files changed; {} manual occurrences left unchanged",
            plan.replacement_count(),
            plan.changed_file_count(),
            plan.manual_count(),
        );
    }
    Ok(if plan.manual_count() == 0 { 0 } else { 1 })
}
