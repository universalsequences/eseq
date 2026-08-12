//! Native Mach-O audit of compiled DGen dylibs (impl spec, decision 4 /
//! slice E5).
//!
//! Reimplements `scripts/audit-dgen-dylib.sh` from the dgen repo without
//! subprocesses: that script shells out to `nm`/`otool`/`file`/`strings`
//! (Command Line Tools), which the production compile path must not require.
//! ESeq passes `--skip-inline-audit` to the DGenLisp subprocess and runs this
//! audit on the exact bytes it is about to publish/load instead.
//!
//! The two symbol allowlists are read at audit time from the staged
//! toolchain's `abi/` directory (`AppPaths::dgen_abi_dir`), never compiled
//! in, so a toolchain update moves the audit contract atomically with the
//! compiler that produces the binaries.

use std::collections::BTreeSet;
use std::path::Path;

use object::read::macho::{LoadCommandVariant, MachHeader};
use object::{Endianness, Object, ObjectSymbol};

/// Generous ceiling; real DGen dylibs are tens of kilobytes.
pub const MAX_DYLIB_BYTES: u64 = 32 * 1024 * 1024;

/// Required `LC_BUILD_VERSION` minos, encoded X.Y.Z as nibbles (11.0.0).
const MINIMUM_MACOS: u32 = 11 << 16;

const EXPORTS_ALLOWLIST_FILE: &str = "exports-v1.txt";
const UNDEFINED_ALLOWLIST_FILE: &str = "libsystem-symbols-v1.txt";
const REQUIRED_DEPENDENCY: &str = "/usr/lib/libSystem.B.dylib";

/// Same forbidden-path set as the shell audit's `strings` check: developer
/// tool installs, user home dirs, and temp dirs must not be baked into a
/// published artifact.
const FORBIDDEN_PATH_STRINGS: [&str; 6] = [
    "/Applications/Xcode",
    "/Library/Developer/CommandLineTools",
    "/usr/bin/clang",
    "/Users/",
    "/private/var/",
    "/tmp/",
];

/// Audit `dylib` against the allowlists staged with the app's toolchain.
pub fn audit_dylib(dylib: &Path) -> Result<(), String> {
    audit_dylib_with_abi_dir(dylib, &crate::app_paths::app_paths().dgen_abi_dir())
}

/// Audit `dylib` against the allowlists in `abi_dir`. Every failed check
/// yields a distinct `dgen-audit[<check>]` line; all failures are reported.
pub fn audit_dylib_with_abi_dir(dylib: &Path, abi_dir: &Path) -> Result<(), String> {
    let failures = collect_audit_failures(dylib, abi_dir)?;
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "DGen ABI v1 audit failed for {}:\n{}",
            dylib.display(),
            failures.join("\n")
        ))
    }
}

fn check_error(check: &str, detail: String) -> String {
    format!("dgen-audit[{check}]: {detail}")
}

/// `Err` is an environment problem (unreadable file/allowlist); `Ok(vec)`
/// holds the audit verdict.
fn collect_audit_failures(dylib: &Path, abi_dir: &Path) -> Result<Vec<String>, String> {
    let expected_exports = read_allowlist(&abi_dir.join(EXPORTS_ALLOWLIST_FILE))?;
    let allowed_undefined = read_allowlist(&abi_dir.join(UNDEFINED_ALLOWLIST_FILE))?;

    let data = std::fs::read(dylib)
        .map_err(|e| format!("dgen audit: read {}: {e}", dylib.display()))?;

    let mut failures = Vec::new();

    if data.len() as u64 > MAX_DYLIB_BYTES {
        failures.push(check_error(
            "file-size",
            format!(
                "dylib is {} bytes; limit is {} bytes",
                data.len(),
                MAX_DYLIB_BYTES
            ),
        ));
    }

    // Header: 64-bit Mach-O, arm64, MH_DYLIB. A fat binary, 32-bit file, or
    // non-Mach-O fails the parse itself.
    let header = match object::macho::MachHeader64::<Endianness>::parse(&*data, 0) {
        Ok(header) => header,
        Err(e) => {
            failures.push(check_error(
                "mach-o",
                format!("not a 64-bit Mach-O file: {e}"),
            ));
            return Ok(failures);
        }
    };
    let endian = match header.endian() {
        Ok(endian) => endian,
        Err(e) => {
            failures.push(check_error("mach-o", format!("bad endianness: {e}")));
            return Ok(failures);
        }
    };

    if header.cputype(endian) != object::macho::CPU_TYPE_ARM64 {
        failures.push(check_error(
            "arch",
            format!(
                "expected arm64 (cputype {:#x}), got cputype {:#x}",
                object::macho::CPU_TYPE_ARM64,
                header.cputype(endian)
            ),
        ));
    }
    if header.filetype(endian) != object::macho::MH_DYLIB {
        failures.push(check_error(
            "filetype",
            format!(
                "expected MH_DYLIB ({:#x}), got filetype {:#x}",
                object::macho::MH_DYLIB,
                header.filetype(endian)
            ),
        ));
    }

    // Load commands: minos, install name, dylib dependencies, LC_RPATH.
    let mut minos: Option<u32> = None;
    let mut install_name: Option<String> = None;
    let mut dependencies: Vec<String> = Vec::new();
    let mut rpath_count = 0usize;

    let mut commands = match header.load_commands(endian, &*data, 0) {
        Ok(commands) => commands,
        Err(e) => {
            failures.push(check_error(
                "load-commands",
                format!("unreadable load commands: {e}"),
            ));
            return Ok(failures);
        }
    };
    loop {
        let command = match commands.next() {
            Ok(Some(command)) => command,
            Ok(None) => break,
            Err(e) => {
                failures.push(check_error(
                    "load-commands",
                    format!("unreadable load command: {e}"),
                ));
                return Ok(failures);
            }
        };
        let variant = match command.variant() {
            Ok(variant) => variant,
            Err(e) => {
                failures.push(check_error(
                    "load-commands",
                    format!("unreadable load command payload: {e}"),
                ));
                return Ok(failures);
            }
        };
        match variant {
            LoadCommandVariant::BuildVersion(build) => {
                // The audited contract is macOS-only; any platform's minos
                // is compared (platform mismatches surface as arch/deps
                // failures anyway).
                minos = Some(build.minos.get(endian));
            }
            LoadCommandVariant::VersionMin(version) => {
                // Legacy pre-LC_BUILD_VERSION encoding.
                if minos.is_none() {
                    minos = Some(version.version.get(endian));
                }
            }
            LoadCommandVariant::IdDylib(id) => {
                if let Ok(name) = command.string(endian, id.dylib.name) {
                    install_name = Some(String::from_utf8_lossy(name).into_owned());
                }
            }
            LoadCommandVariant::Dylib(dylib_command) => {
                if let Ok(name) = command.string(endian, dylib_command.dylib.name) {
                    dependencies.push(String::from_utf8_lossy(name).into_owned());
                }
            }
            LoadCommandVariant::Rpath(_) => rpath_count += 1,
            _ => {}
        }
    }

    match minos {
        Some(minos) if minos == MINIMUM_MACOS => {}
        Some(minos) => failures.push(check_error(
            "minos",
            format!(
                "expected deployment target {}, got {}",
                format_version(MINIMUM_MACOS),
                format_version(minos)
            ),
        )),
        None => failures.push(check_error(
            "minos",
            "missing LC_BUILD_VERSION / LC_VERSION_MIN load command".to_string(),
        )),
    }

    match install_name.as_deref() {
        Some(name) if name.starts_with("@rpath/") => {}
        Some(name) => failures.push(check_error(
            "install-name",
            format!("install name must be @rpath-relative; found: {name}"),
        )),
        None => failures.push(check_error(
            "install-name",
            "dylib has no LC_ID_DYLIB install name".to_string(),
        )),
    }

    if dependencies.len() != 1 || dependencies[0] != REQUIRED_DEPENDENCY {
        failures.push(check_error(
            "dependencies",
            format!(
                "dylib must depend on exactly {REQUIRED_DEPENDENCY}; found: [{}]",
                dependencies.join(", ")
            ),
        ));
    }

    if rpath_count > 0 {
        failures.push(check_error(
            "rpath",
            format!("LC_RPATH is forbidden in DGen artifacts ({rpath_count} present)"),
        ));
    }

    // Symbol tables: exports must match the ABI exactly; undefineds must be a
    // subset of the libSystem allowlist.
    match object::read::macho::MachOFile64::<Endianness>::parse(&*data) {
        Ok(file) => {
            let mut exports = BTreeSet::new();
            let mut undefined = BTreeSet::new();
            for symbol in file.symbols() {
                let Ok(name) = symbol.name() else { continue };
                if name.is_empty() {
                    continue;
                }
                if symbol.is_undefined() {
                    undefined.insert(name.to_string());
                } else if symbol.is_global() && symbol.is_definition() {
                    exports.insert(name.to_string());
                }
            }
            if exports != expected_exports {
                let extra: Vec<_> = exports.difference(&expected_exports).cloned().collect();
                let missing: Vec<_> = expected_exports.difference(&exports).cloned().collect();
                failures.push(check_error(
                    "exports",
                    format!(
                        "exported symbols do not exactly match DGen ABI v1 \
                         (unexpected: [{}], missing: [{}])",
                        extra.join(", "),
                        missing.join(", ")
                    ),
                ));
            }
            let unexpected: Vec<_> = undefined.difference(&allowed_undefined).cloned().collect();
            if !unexpected.is_empty() {
                failures.push(check_error(
                    "undefined-symbols",
                    format!(
                        "undefined symbols fall outside the DGen ABI v1 allowlist: [{}]",
                        unexpected.join(", ")
                    ),
                ));
            }
        }
        Err(e) => failures.push(check_error(
            "symbols",
            format!("unreadable symbol table: {e}"),
        )),
    }

    // Path hygiene, mirroring the shell audit's `strings | grep`. The
    // patterns are printable ASCII, so a raw byte scan finds at least
    // everything `strings` would.
    for pattern in FORBIDDEN_PATH_STRINGS {
        if find_subslice(&data, pattern.as_bytes()) {
            failures.push(check_error(
                "forbidden-paths",
                format!("forbidden path string embedded in dylib: {pattern}"),
            ));
        }
    }

    Ok(failures)
}

fn read_allowlist(path: &Path) -> Result<BTreeSet<String>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "dgen audit: read allowlist {} (is the toolchain staged? run \
             ./rebuild_dgenlisp_tool.sh): {e}",
            path.display()
        )
    })?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect())
}

fn format_version(encoded: u32) -> String {
    format!(
        "{}.{}.{}",
        encoded >> 16,
        (encoded >> 8) & 0xff,
        encoded & 0xff
    )
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    // Fixtures are built at test time with the system clang: the
    // CLT-independence constraint is on the production compile path, not on
    // dev-machine tests, and building them here avoids committing binaries.

    fn fixture_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dgen-audit-fixtures-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        dir
    }

    fn abi_dir() -> PathBuf {
        crate::app_paths::app_paths().dgen_abi_dir()
    }

    const GOOD_SOURCE: &str = r#"
__attribute__((visibility("default"))) void dgen_process_v1(void) {}
__attribute__((visibility("default"))) void dgen_set_param_value_v1(void) {}
"#;

    fn build_fixture(name: &str, source: &str, extra_args: &[&str]) -> PathBuf {
        let dir = fixture_dir();
        let c_path = dir.join(format!("{name}.c"));
        let dylib_path = dir.join(format!("{name}.dylib"));
        std::fs::write(&c_path, source).expect("write fixture source");
        let mut command = Command::new("/usr/bin/cc");
        command
            .arg("-dynamiclib")
            .arg("-fvisibility=hidden")
            .arg("-g0")
            .args(["-mmacosx-version-min=11.0", "-o"])
            .arg(&dylib_path)
            .arg(&c_path)
            .args(extra_args);
        if !extra_args.iter().any(|arg| *arg == "-arch") {
            command.args(["-arch", "arm64"]);
        }
        if !extra_args.iter().any(|arg| *arg == "-install_name") {
            command.args(["-install_name", &format!("@rpath/{name}.dylib")]);
        }
        let output = command.output().expect("run system clang for fixture");
        assert!(
            output.status.success(),
            "fixture clang failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        dylib_path
    }

    #[test]
    fn good_fixture_passes() {
        let dylib = build_fixture("good", GOOD_SOURCE, &[]);
        audit_dylib_with_abi_dir(&dylib, &abi_dir()).expect("good fixture must pass the audit");
    }

    #[test]
    fn x86_64_fixture_fails_arch() {
        let dylib = build_fixture("x86_64", GOOD_SOURCE, &["-arch", "x86_64"]);
        let err = audit_dylib_with_abi_dir(&dylib, &abi_dir()).unwrap_err();
        assert!(err.contains("dgen-audit[arch]"), "{err}");
    }

    #[test]
    fn extra_export_fixture_fails_exports() {
        let source = r#"
__attribute__((visibility("default"))) void dgen_process_v1(void) {}
__attribute__((visibility("default"))) void dgen_set_param_value_v1(void) {}
__attribute__((visibility("default"))) void dgen_evil_extra(void) {}
"#;
        let dylib = build_fixture("extra_export", source, &[]);
        let err = audit_dylib_with_abi_dir(&dylib, &abi_dir()).unwrap_err();
        assert!(err.contains("dgen-audit[exports]"), "{err}");
        assert!(err.contains("_dgen_evil_extra"), "{err}");
    }

    #[test]
    fn missing_export_fixture_fails_exports() {
        let source = r#"
__attribute__((visibility("default"))) void dgen_process_v1(void) {}
"#;
        let dylib = build_fixture("missing_export", source, &[]);
        let err = audit_dylib_with_abi_dir(&dylib, &abi_dir()).unwrap_err();
        assert!(err.contains("dgen-audit[exports]"), "{err}");
        assert!(err.contains("_dgen_set_param_value_v1"), "{err}");
    }

    #[test]
    fn extra_framework_fixture_fails_dependencies() {
        let dylib = build_fixture(
            "accelerate",
            GOOD_SOURCE,
            &["-framework", "Accelerate", "-Wl,-needed_framework,Accelerate"],
        );
        let err = audit_dylib_with_abi_dir(&dylib, &abi_dir()).unwrap_err();
        assert!(err.contains("dgen-audit[dependencies]"), "{err}");
        assert!(err.contains("Accelerate"), "{err}");
    }

    #[test]
    fn absolute_install_name_fixture_fails() {
        let dylib = build_fixture(
            "absname",
            GOOD_SOURCE,
            &["-install_name", "/opt/absname.dylib"],
        );
        let err = audit_dylib_with_abi_dir(&dylib, &abi_dir()).unwrap_err();
        assert!(err.contains("dgen-audit[install-name]"), "{err}");
    }

    #[test]
    fn forbidden_path_string_fixture_fails() {
        let source = r#"
__attribute__((visibility("default"))) void dgen_process_v1(void) {}
__attribute__((visibility("default"))) void dgen_set_param_value_v1(void) {}
__attribute__((used, visibility("hidden"))) static const char kBadPath[] =
    "/Library/Developer/CommandLineTools/usr/bin";
"#;
        let dylib = build_fixture("badpath", source, &[]);
        let err = audit_dylib_with_abi_dir(&dylib, &abi_dir()).unwrap_err();
        assert!(err.contains("dgen-audit[forbidden-paths]"), "{err}");
    }

    #[test]
    fn wrong_minos_fixture_fails() {
        let dylib = build_fixture(
            "minos12",
            GOOD_SOURCE,
            &["-mmacosx-version-min=12.0"],
        );
        let err = audit_dylib_with_abi_dir(&dylib, &abi_dir()).unwrap_err();
        assert!(err.contains("dgen-audit[minos]"), "{err}");
        assert!(err.contains("12.0"), "{err}");
    }

    #[test]
    fn non_macho_file_fails() {
        let dir = fixture_dir();
        let path = dir.join("not-a-dylib.dylib");
        std::fs::write(&path, b"definitely not mach-o").unwrap();
        let err = audit_dylib_with_abi_dir(&path, &abi_dir()).unwrap_err();
        assert!(err.contains("dgen-audit[mach-o]"), "{err}");
    }

    /// Cross-check driver (impl spec, risk 6): audits each comma-separated
    /// dylib path in DGEN_AUDIT_CROSSCHECK and prints the verdicts, for
    /// manual agreement runs against dgen's shell-script audit. No-op (and
    /// green) when the env var is unset.
    #[test]
    fn crosscheck_paths_from_env() {
        let Ok(paths) = std::env::var("DGEN_AUDIT_CROSSCHECK") else {
            return;
        };
        for path in paths.split(',').filter(|path| !path.is_empty()) {
            match audit_dylib_with_abi_dir(std::path::Path::new(path), &abi_dir()) {
                Ok(()) => println!("RUST-AUDIT PASS {path}"),
                Err(error) => println!("RUST-AUDIT FAIL {path}\n{error}"),
            }
        }
    }

    #[test]
    fn real_compiled_effect_passes_audit() {
        // The end-to-end guarantee: a dylib produced by the actual embedded
        // compile path satisfies the audit this module enforces on it.
        let manifest = crate::lisp_host::dgen::effect_compile::compile_lisp(
            "(def sig (in 1))\n(out (* sig 0.5) 1)",
            48000,
        )
        .expect("compile simple effect");
        let manifest = crate::lisp_host::dgen::dgen_manifest::parse_manifest(&manifest)
            .expect("parse manifest");
        audit_dylib_with_abi_dir(&manifest.dylib_path, &abi_dir())
            .expect("real compiled effect must pass the audit");
    }
}
