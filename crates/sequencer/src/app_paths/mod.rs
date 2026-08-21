/*!
Process-wide filesystem layout for the DGen compile path
(docs/embedded-dgen-connector-impl-spec.md, decision 5 / slice E1).

Every location the DGen toolchain touches — the DGenLisp helper binary, the
staged toolchain, effect/instrument source roots, the dylib cache, and the
scratch dirs — resolves through [`AppPaths`], from roots captured once at
construction. Nothing here consults `std::env::current_dir()` after
construction, so the compile path works from any working directory. Other
subsystems still rely on `paths::enter_sequencer_dir()`'s chdir; that stays
untouched.

In dev mode every query resolves to exactly what the pre-`AppPaths` code
resolved to when running from the workspace (post `enter_sequencer_dir()`).
The release arm carries the `.app` bundle shapes from the parent product spec
(`embedded-dgen-toolchain-v0.1-spec.md`, "Writable Runtime Layout") but is
unexercised until Phase 5.

Dev-only override: `ESEQ_DGEN_TOOLCHAIN_ROOT=/abs/path` redirects
[`AppPaths::dgen_toolchain_root`] away from the per-checkout
`tools/dgen-toolchain/` stage. The stage is gitignored (~147 MB), so git
worktrees don't inherit it; the override lets worktrees share the main
checkout's stage. It is captured once at construction, validated (a missing
directory is loudly reported, never silently ignored), and always logged when
active — per the parent spec's "Development Mode" rules. The release arm
ignores it.
*/

use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Clone, Debug)]
pub enum AppPaths {
    /// Cargo-workspace layout: tools and source roots under the sequencer
    /// crate dir, the dylib cache under the workspace root, scratch under the
    /// system temp dir.
    Dev {
        sequencer_dir: PathBuf,
        workspace_root: PathBuf,
        temp_dir: PathBuf,
        /// `ESEQ_DGEN_TOOLCHAIN_ROOT` override, captured at construction (see
        /// module doc). `None` = the default per-checkout stage.
        dgen_toolchain_override: Option<PathBuf>,
    },
    /// `.app` bundle layout per the parent spec: helper binaries in
    /// `Contents/MacOS`, the staged toolchain in `Contents/Resources`,
    /// sources in `~/Library/Application Support/<bundle-id>/`, generated
    /// artifacts in `~/Library/Caches/<bundle-id>/`.
    ///
    /// Phase 5 (parent spec): nothing constructs this yet; the shapes exist
    /// so the release layout is pinned now.
    Release {
        contents_macos: PathBuf,
        contents_resources: PathBuf,
        application_support: PathBuf,
        caches: PathBuf,
    },
}

impl AppPaths {
    pub fn dev(sequencer_dir: PathBuf, workspace_root: PathBuf) -> Self {
        AppPaths::Dev {
            sequencer_dir,
            workspace_root,
            temp_dir: std::env::temp_dir(),
            dgen_toolchain_override: None,
        }
    }

    /// Dev construction that also captures the `ESEQ_DGEN_TOOLCHAIN_ROOT`
    /// override from the environment (see module doc). Used by [`init_dev`]
    /// and the [`app_paths`] fallback; [`Self::dev`] stays env-free for
    /// tests.
    pub fn dev_from_env(sequencer_dir: PathBuf, workspace_root: PathBuf) -> Self {
        let mut paths = Self::dev(sequencer_dir, workspace_root);
        if let Some(override_root) = dev_toolchain_override_from_env() {
            let AppPaths::Dev {
                dgen_toolchain_override,
                ..
            } = &mut paths
            else {
                unreachable!("Self::dev constructs the Dev arm");
            };
            *dgen_toolchain_override = Some(override_root);
        }
        paths
    }

    /// Phase 5 only: deriving these roots (bundle location, `<bundle-id>`
    /// dirs) is unimplemented; no production code constructs this arm yet.
    pub fn release(
        contents_macos: PathBuf,
        contents_resources: PathBuf,
        application_support: PathBuf,
        caches: PathBuf,
    ) -> Self {
        AppPaths::Release {
            contents_macos,
            contents_resources,
            application_support,
            caches,
        }
    }

    /// The DGenLisp helper binary invoked for every compile.
    pub fn dgenlisp_tool(&self) -> PathBuf {
        match self {
            AppPaths::Dev { sequencer_dir, .. } => sequencer_dir.join("tools/DGenLisp"),
            AppPaths::Release { contents_macos, .. } => contents_macos.join("DGenLisp"),
        }
    }

    /// Root of the staged hermetic clang/lld toolchain handed to DGenLisp via
    /// `--toolchain-root`. Dev: staged by `rebuild_dgenlisp_tool.sh` (slice
    /// E2); the directory does not exist until that slice lands.
    pub fn dgen_toolchain_root(&self) -> PathBuf {
        match self {
            AppPaths::Dev {
                sequencer_dir,
                dgen_toolchain_override,
                ..
            } => dgen_toolchain_override
                .clone()
                .unwrap_or_else(|| sequencer_dir.join("tools/dgen-toolchain")),
            AppPaths::Release {
                contents_resources, ..
            } => contents_resources.join("dgen-toolchain"),
        }
    }

    /// [`Self::dgen_toolchain_root`], preflight-checked for the two staged
    /// executables the compile cannot run without. There is deliberately no
    /// fallback to a system compiler (parent spec, Locked Principle 1): a
    /// missing or incomplete stage is a hard, actionable error.
    pub fn dgen_toolchain_root_checked(&self) -> Result<PathBuf, String> {
        let root = self.dgen_toolchain_root();
        let overridden = matches!(
            self,
            AppPaths::Dev {
                dgen_toolchain_override: Some(_),
                ..
            }
        );
        let hint = if overridden {
            "The stage comes from the ESEQ_DGEN_TOOLCHAIN_ROOT override; point it at a \
             complete stage (or unset it and run ./rebuild_dgenlisp_tool.sh to stage one \
             in this checkout)."
        } else {
            "Run ./rebuild_dgenlisp_tool.sh at the repo root to stage it (the stage is \
             gitignored, so fresh checkouts and worktrees start without one)."
        };
        if !root.is_dir() {
            return Err(format!(
                "DGen toolchain stage not found at {}. {hint}",
                root.display()
            ));
        }
        for rel in ["bin/dgen-clang", "bin/ld64.lld"] {
            if !root.join(rel).is_file() {
                return Err(format!(
                    "DGen toolchain stage at {} is incomplete (missing {rel}). {hint}",
                    root.display()
                ));
            }
        }
        Ok(root)
    }

    /// ABI allowlist dir (`exports-v1.txt`, `libsystem-symbols-v1.txt`) read
    /// by the Rust binary audit (slice E5).
    pub fn dgen_abi_dir(&self) -> PathBuf {
        self.dgen_toolchain_root().join("abi")
    }

    /// Checked-in project fixtures used by the end-to-end performance probes.
    /// These are deliberately separate from the user's mutable project library.
    pub fn perf_probe_projects_dir(&self) -> PathBuf {
        match self {
            AppPaths::Dev { sequencer_dir, .. } => {
                sequencer_dir.join("tests/fixtures/projects")
            }
            AppPaths::Release {
                contents_resources, ..
            } => contents_resources.join("test-fixtures/projects"),
        }
    }

    /// Saved effect sources ("effects" tree; the user-visible/serialized
    /// names stay relative to this root).
    pub fn effects_dir(&self) -> PathBuf {
        match self {
            AppPaths::Dev { sequencer_dir, .. } => sequencer_dir.join("effects"),
            AppPaths::Release {
                application_support,
                ..
            } => application_support.join("effects"),
        }
    }

    /// Saved instrument sources ("instruments" tree).
    pub fn instruments_dir(&self) -> PathBuf {
        match self {
            AppPaths::Dev { sequencer_dir, .. } => sequencer_dir.join("instruments"),
            AppPaths::Release {
                application_support,
                ..
            } => application_support.join("instruments"),
        }
    }

    /// Base for resolving relative `@file` asset references when a compile
    /// supplies no explicit asset base. Dev: the sequencer crate dir (what
    /// `current_dir()` was after `enter_sequencer_dir()`).
    pub fn dgen_asset_fallback_base(&self) -> PathBuf {
        match self {
            AppPaths::Dev { sequencer_dir, .. } => sequencer_dir.clone(),
            AppPaths::Release {
                application_support,
                ..
            } => application_support.clone(),
        }
    }

    /// Root of the content-addressed compiled-dylib cache.
    pub fn dgen_cache_root(&self) -> PathBuf {
        match self {
            AppPaths::Dev { workspace_root, .. } => {
                workspace_root.join(".eseq").join("dgenlisp-cache")
            }
            AppPaths::Release { caches, .. } => caches.join("dgen"),
        }
    }

    /// Persistent patch-learning job artifacts. These are deliberately kept
    /// outside the transient compiler scratch directory: a completed or
    /// interrupted job is useful after restart for replay and diagnosis.
    pub fn learn_jobs_dir(&self) -> PathBuf {
        match self {
            AppPaths::Dev { workspace_root, .. } => workspace_root.join(".eseq").join("learn-jobs"),
            AppPaths::Release {
                application_support,
                ..
            } => application_support.join("learn-jobs"),
        }
    }

    /// Output dir for uncached compiles (the pre-cache `output_dir()`).
    pub fn dgen_scratch_dir(&self) -> PathBuf {
        match self {
            AppPaths::Dev { temp_dir, .. } => temp_dir.join("sequencer_dgenlisp"),
            AppPaths::Release { caches, .. } => caches.join("dgen-scratch"),
        }
    }

    /// Scratch dir for convolution-reverb IR partitioning.
    pub fn ir_prep_dir(&self) -> PathBuf {
        match self {
            AppPaths::Dev { temp_dir, .. } => temp_dir.join("sequencer_ir_prep"),
            AppPaths::Release { caches, .. } => caches.join("ir-prep"),
        }
    }
}

/// Read and validate the dev-only `ESEQ_DGEN_TOOLCHAIN_ROOT` override.
/// Always loud: an active override is logged, and a set-but-missing path is
/// reported (and still honored, so the compile preflight hard-errors against
/// the path the user asked for instead of silently using another toolchain).
fn dev_toolchain_override_from_env() -> Option<PathBuf> {
    let raw = std::env::var_os("ESEQ_DGEN_TOOLCHAIN_ROOT")?;
    if raw.is_empty() {
        return None;
    }
    let root = PathBuf::from(raw);
    if root.is_dir() {
        eprintln!(
            "[app_paths] ESEQ_DGEN_TOOLCHAIN_ROOT override active: dgen toolchain root = {}",
            root.display()
        );
    } else {
        eprintln!(
            "[app_paths] ESEQ_DGEN_TOOLCHAIN_ROOT is set but {} is not a directory; \
             DGen compiles will fail until it points at a staged toolchain",
            root.display()
        );
    }
    Some(root)
}

static APP_PATHS: OnceLock<AppPaths> = OnceLock::new();

/// Install the dev-layout `AppPaths` for this process. Called at startup next
/// to `paths::enter_sequencer_dir()`, while the workspace is still locatable.
/// Idempotent; later calls keep the first installation.
pub fn init_dev() -> io::Result<()> {
    let paths = AppPaths::dev_from_env(
        crate::paths::sequencer_dir()?,
        crate::paths::workspace_root(),
    );
    let _ = APP_PATHS.set(paths);
    Ok(())
}

/// Process-wide accessor. Falls back to a dev-layout construction when
/// `init_dev()` was never called (tests, helper tools); the fallback locates
/// the workspace once and caches the result — queries never re-consult the
/// working directory.
pub fn app_paths() -> &'static AppPaths {
    APP_PATHS.get_or_init(|| {
        let sequencer_dir = crate::paths::sequencer_dir()
            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        AppPaths::dev_from_env(sequencer_dir, crate::paths::workspace_root())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_paths_resolve_from_captured_roots_only() {
        let paths = AppPaths::dev(PathBuf::from("/ws/crates/sequencer"), PathBuf::from("/ws"));
        assert_eq!(
            paths.dgenlisp_tool(),
            PathBuf::from("/ws/crates/sequencer/tools/DGenLisp")
        );
        assert_eq!(
            paths.dgen_toolchain_root(),
            PathBuf::from("/ws/crates/sequencer/tools/dgen-toolchain")
        );
        assert_eq!(
            paths.dgen_abi_dir(),
            PathBuf::from("/ws/crates/sequencer/tools/dgen-toolchain/abi")
        );
        assert_eq!(
            paths.perf_probe_projects_dir(),
            PathBuf::from("/ws/crates/sequencer/tests/fixtures/projects")
        );
        assert_eq!(
            paths.effects_dir(),
            PathBuf::from("/ws/crates/sequencer/effects")
        );
        assert_eq!(
            paths.instruments_dir(),
            PathBuf::from("/ws/crates/sequencer/instruments")
        );
        assert_eq!(
            paths.dgen_asset_fallback_base(),
            PathBuf::from("/ws/crates/sequencer")
        );
        assert_eq!(
            paths.dgen_cache_root(),
            PathBuf::from("/ws/.eseq/dgenlisp-cache")
        );
        assert_eq!(
            paths.learn_jobs_dir(),
            PathBuf::from("/ws/.eseq/learn-jobs")
        );
        assert_eq!(
            paths.dgen_scratch_dir(),
            std::env::temp_dir().join("sequencer_dgenlisp")
        );
        assert_eq!(
            paths.ir_prep_dir(),
            std::env::temp_dir().join("sequencer_ir_prep")
        );
    }

    #[test]
    fn perf_probe_project_fixtures_are_present_and_parse() {
        let fixture_dir = app_paths().perf_probe_projects_dir();
        for name in ["92", "pianohold", "arrtest3"] {
            let path = fixture_dir.join(format!("{name}.json"));
            assert!(
                path.is_file(),
                "perf probe fixture not found: {}",
                path.display()
            );
            crate::project::load_project_from_path(&path).unwrap_or_else(|error| {
                panic!(
                    "failed to load perf probe fixture '{}': {error}",
                    path.display()
                )
            });
        }
    }

    #[test]
    fn release_perf_probe_projects_resolve_from_resources() {
        let paths = AppPaths::release(
            PathBuf::from("/App/Contents/MacOS"),
            PathBuf::from("/App/Contents/Resources"),
            PathBuf::from("/Support"),
            PathBuf::from("/Caches"),
        );
        assert_eq!(
            paths.perf_probe_projects_dir(),
            PathBuf::from("/App/Contents/Resources/test-fixtures/projects")
        );
    }

    #[test]
    fn toolchain_override_redirects_root_and_preflight_reports_it() {
        let mut paths = AppPaths::dev(PathBuf::from("/ws/crates/sequencer"), PathBuf::from("/ws"));
        let AppPaths::Dev {
            dgen_toolchain_override,
            ..
        } = &mut paths
        else {
            unreachable!()
        };
        *dgen_toolchain_override = Some(PathBuf::from("/shared/dgen-toolchain"));
        assert_eq!(
            paths.dgen_toolchain_root(),
            PathBuf::from("/shared/dgen-toolchain")
        );
        let err = paths
            .dgen_toolchain_root_checked()
            .expect_err("missing override dir must be a hard error");
        assert!(err.contains("/shared/dgen-toolchain"), "{err}");
        assert!(err.contains("ESEQ_DGEN_TOOLCHAIN_ROOT"), "{err}");
    }

    #[test]
    fn missing_default_stage_error_mentions_rebuild_script() {
        let paths = AppPaths::dev(
            PathBuf::from("/nonexistent/crates/sequencer"),
            PathBuf::from("/nonexistent"),
        );
        let err = paths
            .dgen_toolchain_root_checked()
            .expect_err("missing stage must be a hard error");
        assert!(err.contains("rebuild_dgenlisp_tool.sh"), "{err}");
        assert!(err.contains("tools/dgen-toolchain"), "{err}");
    }

    #[test]
    fn global_accessor_matches_workspace_layout() {
        let paths = app_paths();
        let sequencer_dir = crate::paths::sequencer_dir().expect("locate sequencer dir");
        assert_eq!(paths.dgenlisp_tool(), sequencer_dir.join("tools/DGenLisp"));
        assert_eq!(
            paths.dgen_cache_root(),
            crate::paths::workspace_root()
                .join(".eseq")
                .join("dgenlisp-cache")
        );
    }
}
