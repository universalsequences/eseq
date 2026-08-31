/*!
Process-wide filesystem layout for the DGen compile path
(docs/embedded-dgen-connector-impl-spec.md, decision 5 / slice E1).

Every location the DGen toolchain touches — the DGenLisp helper binary, the
staged toolchain, effect/instrument source roots, the dylib cache, and the
scratch dirs — resolves through [`AppPaths`], from roots captured once at
construction. Nothing here consults `std::env::current_dir()` after
construction, so the compile path works from any working directory. The UI
startup still enters the sequencer directory in development mode, but skips
that checkout-only chdir when this module detects a packaged executable.

In dev mode every query resolves to exactly what the pre-`AppPaths` code
resolved to when running from the workspace (post `enter_sequencer_dir()`).
The release arm carries the packaged application shapes from the parent product
spec (`embedded-dgen-toolchain-v0.1-spec.md`, "Writable Runtime Layout") but is
unexercised until Phase 5.

Dev-only override: `ESEQ_DGENLISP_TOOL=/abs/path` selects a custom DGenLisp
compiler. Like all dev path overrides, it is captured at construction and
logged when active. The default compiler filename is selected from the build
target, so a checkout containing tools for multiple targets never executes a
host-incompatible binary. The default compiler itself is not tracked in git:
`content/dgenlisp.lock` pins the published distribution per target and
`scripts/fetch_dgenlisp.sh` installs it at the resolved path (see
[`AppPaths::dgenlisp_tool_checked`]).

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
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const DGENLISP_TOOL_FILENAME: &str = "DGenLisp-macos-arm64";
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const DGEN_TOOLCHAIN_TARGET: &str = "arm64-apple-macos";
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const DGEN_TOOLCHAIN_REQUIRED_FILES: &[&str] = &["bin/dgen-clang", "bin/ld64.lld"];
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const DGENLISP_TOOL_FILENAME: &str = "DGenLisp-linux-x86_64";
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const DGEN_TOOLCHAIN_TARGET: &str = "x86_64-unknown-linux-gnu";
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const DGEN_TOOLCHAIN_REQUIRED_FILES: &[&str] = &["bin/dgen-clang"];
#[cfg(not(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64")
)))]
compile_error!("no bundled DGenLisp compiler exists for this target");

#[derive(Clone, Debug)]
pub enum AppPaths {
    /// Cargo-workspace layout: tools and source roots under the sequencer
    /// crate dir, the dylib cache under the workspace root, scratch under the
    /// system temp dir.
    Dev {
        sequencer_dir: PathBuf,
        workspace_root: PathBuf,
        temp_dir: PathBuf,
        user_lisp_root: PathBuf,
        /// `ESEQ_DGENLISP_TOOL` override, captured at construction (see module
        /// doc). `None` selects the checked-in compiler for the build target.
        dgenlisp_tool_override: Option<PathBuf>,
        /// `ESEQ_DGEN_TOOLCHAIN_ROOT` override, captured at construction (see
        /// module doc). `None` = the default per-checkout stage.
        dgen_toolchain_override: Option<PathBuf>,
    },
    /// Packaged layout per the parent spec: helper binaries in the platform's
    /// executable directory, staged resources in `contents_resources`, and
    /// mutable user/cache roots supplied by the platform launcher.
    ///
    Release {
        executable_dir: PathBuf,
        contents_resources: PathBuf,
        application_support: PathBuf,
        caches: PathBuf,
        user_lisp_root: PathBuf,
    },
}

impl AppPaths {
    pub fn dev(
        sequencer_dir: PathBuf,
        workspace_root: PathBuf,
        user_lisp_root: PathBuf,
    ) -> Self {
        AppPaths::Dev {
            sequencer_dir,
            workspace_root,
            temp_dir: std::env::temp_dir(),
            user_lisp_root,
            dgenlisp_tool_override: None,
            dgen_toolchain_override: None,
        }
    }

    /// Dev construction that captures user and toolchain overrides from the
    /// environment. `ESEQ_CONFIG_DIR` redirects the hand-edited user Lisp
    /// tier; otherwise it is `~/.eseq.d`. Both roots are captured once so no
    /// filesystem query can observe a different environment mid-process.
    pub fn dev_from_env(sequencer_dir: PathBuf, workspace_root: PathBuf) -> io::Result<Self> {
        let mut paths = Self::dev(
            sequencer_dir,
            workspace_root,
            user_lisp_root_from_env()?,
        );
        let AppPaths::Dev {
            dgenlisp_tool_override,
            dgen_toolchain_override,
            ..
        } = &mut paths
        else {
            unreachable!("Self::dev constructs the Dev arm");
        };
        *dgenlisp_tool_override = dev_dgenlisp_tool_override_from_env();
        *dgen_toolchain_override = dev_toolchain_override_from_env();
        Ok(paths)
    }

    pub fn release(
        executable_dir: PathBuf,
        contents_resources: PathBuf,
        application_support: PathBuf,
        caches: PathBuf,
        user_lisp_root: PathBuf,
    ) -> Self {
        AppPaths::Release {
            executable_dir,
            contents_resources,
            application_support,
            caches,
            user_lisp_root,
        }
    }

    pub fn is_release(&self) -> bool {
        matches!(self, AppPaths::Release { .. })
    }

    /// The DGenLisp helper binary invoked for every compile.
    pub fn dgenlisp_tool(&self) -> PathBuf {
        match self {
            AppPaths::Dev {
                sequencer_dir,
                dgenlisp_tool_override,
                ..
            } => dgenlisp_tool_override
                .clone()
                .unwrap_or_else(|| sequencer_dir.join("tools").join(DGENLISP_TOOL_FILENAME)),
            AppPaths::Release { executable_dir, .. } => {
                executable_dir.join(DGENLISP_TOOL_FILENAME)
            }
        }
    }

    /// [`Self::dgenlisp_tool`], preflight-checked for existence. The compiler
    /// is not tracked in git — `content/dgenlisp.lock` pins the published
    /// distribution and `scripts/fetch_dgenlisp.sh` installs it — so a fresh
    /// checkout starts without one and the absence must be a hard, actionable
    /// error naming the exact fetch command, never a bare spawn failure.
    pub fn dgenlisp_tool_checked(&self) -> Result<PathBuf, String> {
        let tool = self.dgenlisp_tool();
        if tool.is_file() {
            return Ok(tool);
        }
        let hint = match self {
            AppPaths::Dev {
                dgenlisp_tool_override: Some(_),
                ..
            } => {
                "The path comes from the ESEQ_DGENLISP_TOOL override; point it at a real \
                 compiler, or unset it and run ./scripts/fetch_dgenlisp.sh at the repo root \
                 to install the distribution pinned in content/dgenlisp.lock."
            }
            AppPaths::Dev { .. } => {
                "Run ./scripts/fetch_dgenlisp.sh at the repo root to install the \
                 distribution pinned in content/dgenlisp.lock (the compiler is gitignored, \
                 so fresh checkouts and worktrees start without one)."
            }
            AppPaths::Release { .. } => {
                "The packaged application is missing its bundled compiler; reinstall it."
            }
        };
        Err(format!(
            "DGenLisp compiler not found at {}. {hint}",
            tool.display()
        ))
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

    /// [`Self::dgen_toolchain_root`], preflight-checked for this host target.
    /// There is deliberately no fallback to a system compiler (parent spec,
    /// Locked Principle 1): a missing, incomplete, or wrong-target stage is a
    /// hard, actionable error.
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
             complete stage (or unset it and stage one in this checkout with \
             ./scripts/fetch_dgen_toolchain.sh for published targets, or \
             ./rebuild_dgenlisp_tool.sh for targets vendored from a local dgen-audio \
             checkout)."
        } else {
            "Run ./scripts/fetch_dgen_toolchain.sh at the repo root to stage it (published \
             targets), or ./rebuild_dgenlisp_tool.sh for targets vendored from a local \
             dgen-audio checkout. The stage is gitignored, so fresh checkouts and worktrees \
             start without one."
        };
        if !root.is_dir() {
            return Err(format!(
                "DGen toolchain stage not found at {}. {hint}",
                root.display()
            ));
        }
        let version_path = root.join("VERSION.json");
        let version = std::fs::read_to_string(&version_path).map_err(|e| {
            format!(
                "DGen toolchain stage at {} is incomplete (cannot read VERSION.json: {e}). {hint}",
                root.display()
            )
        })?;
        let target = serde_json::from_str::<serde_json::Value>(&version)
            .ok()
            .and_then(|value| value.get("target")?.as_str().map(str::to_owned));
        if target.as_deref() != Some(DGEN_TOOLCHAIN_TARGET) {
            return Err(format!(
                "DGen toolchain stage at {} targets {}, but this host requires {}. \
                 Toolchain stages are target-specific and cannot be reused across architectures. {hint}",
                root.display(),
                target.as_deref().unwrap_or("an unknown target (invalid VERSION.json)"),
                DGEN_TOOLCHAIN_TARGET,
            ));
        }
        for rel in DGEN_TOOLCHAIN_REQUIRED_FILES {
            if !root.join(rel).is_file() {
                return Err(format!(
                    "DGen toolchain stage at {} is incomplete (missing {rel}). {hint}",
                    root.display()
                ));
            }
        }
        Ok(root)
    }

    /// Platform ABI allowlists read by the Rust binary audit (slice E5).
    pub fn dgen_abi_dir(&self) -> PathBuf {
        #[cfg(target_os = "macos")]
        {
            self.dgen_toolchain_root().join("abi")
        }
        #[cfg(target_os = "linux")]
        {
            // Linux distributions carry their compile policy and allowlists
            // beside the real compiler binary. Resolve the fetch script's
            // stable symlink so this works in both dev and packaged layouts.
            let tool = self.dgenlisp_tool();
            let real_tool = std::fs::canonicalize(&tool).unwrap_or(tool);
            real_tool
                .parent()
                .expect("DGenLisp compiler path must have a parent")
                .join("toolchain/abi")
        }
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

    /// Exact monospace face that defines the application's cell grid. It is a
    /// checked-in packaging resource in development and is copied into the app
    /// bundle for release, so layout never depends on a user's installed fonts.
    pub fn ui_monospace_font(&self) -> PathBuf {
        match self {
            AppPaths::Dev { workspace_root, .. } => workspace_root
                .join("dist/macos/fonts/JetBrainsMono-Regular.ttf"),
            AppPaths::Release {
                contents_resources, ..
            } => contents_resources.join("fonts/JetBrainsMono-Regular.ttf"),
        }
    }

    /// Read-only factory content root.
    pub fn factory_root(&self) -> PathBuf {
        match self {
            AppPaths::Dev { workspace_root, .. } => workspace_root.join("content"),
            AppPaths::Release {
                contents_resources, ..
            } => contents_resources.clone(),
        }
    }

    /// Mutable, machine-managed user data.
    pub fn user_data_root(&self) -> PathBuf {
        match self {
            AppPaths::Dev { workspace_root, .. } => workspace_root.join(".local"),
            AppPaths::Release {
                application_support, ..
            } => application_support.clone(),
        }
    }

    /// Base used to resolve relative scripts persisted in a project's scratch
    /// source. Development preserves the historical workspace-relative forms;
    /// installed projects resolve mutable relative paths from Application
    /// Support, while factory paths remain available through the configured
    /// eseqlisp load fallback roots.
    pub fn project_script_root(&self) -> PathBuf {
        match self {
            AppPaths::Dev { workspace_root, .. } => workspace_root.clone(),
            AppPaths::Release {
                application_support, ..
            } => application_support.clone(),
        }
    }

    /// Synthetic source path associated with a project's persisted scratch
    /// buffer. Its parent deliberately matches [`Self::project_script_root`]
    /// so runtime relative loads and UI path normalization use the same base.
    pub fn project_scratch_source_path(&self) -> PathBuf {
        self.project_script_root().join(".eseqlisp-scratch")
    }

    /// Process-global preferences written by UI features such as the agentic
    /// model picker.
    pub fn preferences_path(&self) -> PathBuf {
        match self {
            AppPaths::Dev { workspace_root, .. } => {
                workspace_root.join(".eseq").join("prefs.json")
            }
            AppPaths::Release {
                application_support, ..
            } => application_support.join("prefs.json"),
        }
    }

    /// Hand-edited user Lisp root. Dev honors `ESEQ_CONFIG_DIR`; release
    /// construction receives the resolved `~/.eseq.d` path explicitly.
    pub fn user_lisp_root(&self) -> &Path {
        match self {
            AppPaths::Dev { user_lisp_root, .. }
            | AppPaths::Release { user_lisp_root, .. } => user_lisp_root,
        }
    }

    /// Manifest-free personal modules. This reserved local workspace lives
    /// beside installed packages but is itself not a distributable package.
    pub fn local_modules_dir(&self) -> PathBuf {
        self.packages_dir().join("local")
    }

    pub fn packages_dir(&self) -> PathBuf {
        self.user_lisp_root().join("packages")
    }

    pub fn factory_packages_dir(&self) -> PathBuf {
        self.factory_root().join("packages")
    }

    /// Validated module roots in shadowing order, plus one message per
    /// package that failed validation. Package roots carry their owned
    /// namespace so `src/ui.lisp` resolves `author.package.ui` without
    /// allowing that package to satisfy somebody else's import. Invalid
    /// packages are excluded and reported — like a failed user init, a broken
    /// third-party clone never aborts application boot.
    pub fn module_load_roots(&self) -> (Vec<eseqlisp::ModuleLoadRoot>, Vec<String>) {
        let (packages, errors) = eseqlisp::package::PackageCatalog::scan_layered_reporting(&[
            self.packages_dir(),
            self.factory_packages_dir(),
        ]);

        let mut roots = vec![eseqlisp::ModuleLoadRoot {
            path: self.local_modules_dir(),
            module_prefix: None,
        }];
        roots.extend(packages.module_roots().into_iter().map(|(path, module_prefix)| {
            eseqlisp::ModuleLoadRoot { path, module_prefix: Some(module_prefix) }
        }));
        roots.push(eseqlisp::ModuleLoadRoot {
            path: self.factory_root(),
            module_prefix: None,
        });
        (roots, errors.into_iter().map(|error| error.to_string()).collect())
    }

    pub fn load_path(&self) -> io::Result<Vec<PathBuf>> {
        Ok(self.module_load_roots().0.into_iter().map(|root| root.path).collect())
    }

    /// Create the complete mutable user-tier directory shape. This is safe to
    /// call on every startup and never creates or overwrites `init.lisp`.
    pub fn ensure_user_tier(&self) -> io::Result<()> {
        for directory in [
            self.user_data_root(),
            self.projects_dir(),
            self.recordings_dir(),
            self.samples_dir(),
            self.sounds_dir(),
            self.user_instruments_dir(),
            self.user_effects_dir(),
            self.user_assets_dir(),
            self.sample_assets_dir(),
            self.user_filter_tables_dir(),
            self.user_presets_dir(),
            self.user_rack_presets_dir(),
            self.user_kits_dir(),
            self.user_lisp_root().to_path_buf(),
            self.packages_dir(),
            self.local_modules_dir(),
        ] {
            std::fs::create_dir_all(directory)?;
        }
        Ok(())
    }

    /// Destination for the low-level crash report installed at startup.
    ///
    /// Dev keeps the historical checkout-relative name — startup has already
    /// chdir'd into the crate directory by then. Release must be absolute:
    /// Finder launches a bundle with the working directory set to `/`, so
    /// opening a relative path there fails with `ReadOnlyFilesystem` and takes
    /// the whole process down before a window ever opens. The directory is
    /// `user_data_root()`, which [`Self::ensure_user_tier`] has already made.
    pub fn crash_log_path(&self) -> PathBuf {
        match self {
            AppPaths::Dev { .. } => PathBuf::from(crate::crash::CRASH_LOG_FILENAME),
            AppPaths::Release { .. } => self
                .user_data_root()
                .join(crate::crash::CRASH_LOG_FILENAME),
        }
    }

    /// Core eseqlisp shipped with the application.
    pub fn core_dir(&self) -> PathBuf {
        self.factory_root().join("core")
    }

    pub fn ui_dir(&self) -> PathBuf {
        self.factory_root().join("ui")
    }
    pub fn defmacros_dir(&self) -> PathBuf {
        self.factory_root().join("defmacros")
    }
    pub fn midi_fx_dir(&self) -> PathBuf {
        self.factory_root().join("midi-fx")
    }
    pub fn presets_dir(&self) -> PathBuf {
        self.factory_root().join("presets")
    }
    pub fn rack_presets_dir(&self) -> PathBuf {
        self.presets_dir().join("racks")
    }
    pub fn kits_dir(&self) -> PathBuf {
        self.factory_root().join("kits")
    }
    pub fn processes_dir(&self) -> PathBuf {
        self.factory_root().join("processes")
    }
    pub fn scripts_dir(&self) -> PathBuf {
        self.factory_root().join("scripts")
    }
    pub fn impulses_dir(&self) -> PathBuf {
        self.factory_root().join("impulses")
    }
    pub fn filter_tables_dir(&self) -> PathBuf {
        self.factory_root().join("filter-tables")
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.user_data_root().join("projects")
    }
    pub fn recordings_dir(&self) -> PathBuf {
        self.user_data_root().join("recordings")
    }
    pub fn samples_dir(&self) -> PathBuf {
        self.user_data_root().join("samples")
    }
    pub fn sample_db_path(&self) -> PathBuf {
        self.user_data_root().join("samples.db")
    }
    pub fn sample_facts_path(&self) -> PathBuf {
        self.user_data_root().join("samples.jsonl")
    }

    /// Resolve the portable sample identity persisted in projects and browser
    /// rows against this layout's sample store.
    ///
    /// `samples/<sha256>.wav` is an identity, not a path relative to the
    /// process working directory. Resolve that shape unconditionally so an
    /// obsolete checkout-local `samples/` directory can never shadow the
    /// configured store, and so diagnostics for missing samples name the
    /// location that was actually expected. Absolute and other relative paths
    /// remain available for imported files and fixtures.
    pub fn resolve_sample_ref(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            return path.to_path_buf();
        }

        if let Ok(relative) = path.strip_prefix("samples") {
            let mut components = relative.components();
            if let (Some(std::path::Component::Normal(name)), None) =
                (components.next(), components.next())
            {
                return self.samples_dir().join(name);
            }
        }

        path.to_path_buf()
    }

    pub fn sounds_dir(&self) -> PathBuf {
        self.user_data_root().join("sounds")
    }
    pub fn sample_assets_dir(&self) -> PathBuf {
        self.user_data_root().join("sample-assets")
    }
    pub fn user_filter_tables_dir(&self) -> PathBuf {
        self.user_data_root().join("filter-tables")
    }
    pub fn user_presets_dir(&self) -> PathBuf {
        self.user_data_root().join("presets")
    }
    pub fn user_rack_presets_dir(&self) -> PathBuf {
        self.user_presets_dir().join("racks")
    }
    pub fn user_kits_dir(&self) -> PathBuf {
        self.user_data_root().join("kits")
    }

    /// Factory effect and instrument trees are immutable. Authoring paths are
    /// always in the mutable user-data tier.
    pub fn effects_dir(&self) -> PathBuf {
        self.factory_root().join("effects")
    }
    pub fn instruments_dir(&self) -> PathBuf {
        self.factory_root().join("instruments")
    }
    pub fn user_effects_dir(&self) -> PathBuf {
        self.user_data_root().join("effects")
    }
    pub fn user_instruments_dir(&self) -> PathBuf {
        self.user_data_root().join("instruments")
    }
    /// Immutable assets distributed with the application.
    pub fn factory_assets_dir(&self) -> PathBuf {
        self.factory_root().join("assets")
    }
    /// Mutable shared assets installed by the user.
    pub fn user_assets_dir(&self) -> PathBuf {
        self.user_data_root().join("assets")
    }

    pub fn effect_dirs(&self) -> Vec<PathBuf> {
        distinct_paths([self.effects_dir(), self.user_effects_dir()])
    }

    pub fn instrument_dirs(&self) -> Vec<PathBuf> {
        distinct_paths([self.instruments_dir(), self.user_instruments_dir()])
    }

    /// Base for resolving relative `@file` asset references when a compile
    /// supplies no explicit asset base.
    pub fn dgen_asset_fallback_base(&self) -> PathBuf {
        match self {
            AppPaths::Dev { .. } => self.factory_root(),
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

    /// Persistent prepared convolution IRs. This is distinct from
    /// [`Self::ir_prep_dir`], which holds transient partitioning scratch.
    pub fn convolution_ir_cache_dir(&self) -> PathBuf {
        self.dgen_cache_root().join("ir-prep")
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
fn distinct_paths<const N: usize>(paths: [PathBuf; N]) -> Vec<PathBuf> {
    distinct_vec_paths(paths.into_iter().collect())
}

fn distinct_vec_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for path in paths {
        if !result.contains(&path) {
            result.push(path);
        }
    }
    result
}

fn user_lisp_root_from_env() -> io::Result<PathBuf> {
    let config_override = std::env::var_os("ESEQ_CONFIG_DIR");
    let root = resolve_user_lisp_root(config_override.clone(), std::env::var_os("HOME"))?;
    if config_override.is_some_and(|value| !value.is_empty()) {
        eprintln!(
            "[app_paths] ESEQ_CONFIG_DIR override active: user Lisp root = {}",
            root.display()
        );
    }
    Ok(root)
}

fn resolve_user_lisp_root(
    config_override: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> io::Result<PathBuf> {
    if let Some(root) = config_override.filter(|root| !root.is_empty()) {
        return Ok(PathBuf::from(root));
    }
    let home = home.filter(|home| !home.is_empty()).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "HOME is not set; set ESEQ_CONFIG_DIR to choose the user Lisp root",
        )
    })?;
    Ok(PathBuf::from(home).join(".eseq.d"))
}

fn dev_dgenlisp_tool_override_from_env() -> Option<PathBuf> {
    let raw = std::env::var_os("ESEQ_DGENLISP_TOOL")?;
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(raw);
    if path.is_file() {
        eprintln!(
            "[app_paths] ESEQ_DGENLISP_TOOL override active: DGenLisp compiler = {}",
            path.display()
        );
    } else {
        eprintln!(
            "[app_paths] ESEQ_DGENLISP_TOOL is set but {} is not a file; \
             DGen compiles will fail until it points at a compiler executable",
            path.display()
        );
    }
    Some(path)
}

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

/// Resolve a possibly-relative, content-addressed sample reference through
/// the process-wide filesystem layout.
pub fn resolve_sample_ref(path: &std::path::Path) -> PathBuf {
    app_paths().resolve_sample_ref(path)
}

static APP_PATHS: OnceLock<AppPaths> = OnceLock::new();

/// Installed user data and cache directory name. This is the ratified
/// `CFBundleIdentifier` (`dist/macos/build.sh`, release spec section 3.1) and
/// must stay in lockstep with it: `~/Library/Application Support/<bundle-id>/`
/// and `~/Library/Caches/<bundle-id>/` are the conventional macOS locations,
/// and changing the name after a tester has saved a project orphans their
/// library. Decide once, migrate never.
const RELEASE_DATA_DIRECTORY: &str = "com.universalsequences.eseq";

#[derive(Debug, PartialEq, Eq)]
enum RuntimeLayout {
    Dev,
    Release {
        executable_dir: PathBuf,
        contents_resources: PathBuf,
    },
}

/// Select the runtime layout solely from the executable's location. A release
/// executable must be an immediate child of `*.app/Contents/MacOS`; paths that
/// merely contain one of those component names remain development paths.
fn runtime_layout(executable: &Path) -> RuntimeLayout {
    let Some(executable_dir) = executable.parent() else {
        return RuntimeLayout::Dev;
    };
    let Some(contents_dir) = executable_dir.parent() else {
        return RuntimeLayout::Dev;
    };
    let Some(app_dir) = contents_dir.parent() else {
        return RuntimeLayout::Dev;
    };
    let is_bundle = executable_dir.file_name().is_some_and(|name| name == "MacOS")
        && contents_dir.file_name().is_some_and(|name| name == "Contents")
        && app_dir.extension().is_some_and(|extension| extension == "app");
    if !is_bundle {
        return RuntimeLayout::Dev;
    }
    RuntimeLayout::Release {
        executable_dir: executable_dir.to_path_buf(),
        contents_resources: contents_dir.join("Resources"),
    }
}

fn release_paths_for_executable_and_home(
    executable: &Path,
    home: &Path,
) -> Option<AppPaths> {
    let RuntimeLayout::Release {
        executable_dir,
        contents_resources,
    } = runtime_layout(executable)
    else {
        return None;
    };
    Some(AppPaths::release(
        executable_dir,
        contents_resources,
        home.join("Library/Application Support").join(RELEASE_DATA_DIRECTORY),
        home.join("Library/Caches").join(RELEASE_DATA_DIRECTORY),
        home.join(".eseq.d"),
    ))
}

fn release_paths_for_executable(executable: &Path) -> io::Result<Option<AppPaths>> {
    if runtime_layout(executable) == RuntimeLayout::Dev {
        return Ok(None);
    }
    // Release deliberately does not consult ESEQ_CONFIG_DIR or any checkout /
    // toolchain override. Installed roots are fixed relative to HOME and the
    // bundle so a developer environment cannot redirect a shipped app.
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(release_paths_for_executable_and_home(executable, &home))
}

/// Install `paths` as the process layout, or fail loudly if a different arm
/// was already installed. `APP_PATHS` is also lazily filled by [`app_paths`]
/// with a dev fallback, so a query that runs before startup would otherwise
/// leave a bundled app silently resolving to the build machine's checkout —
/// the exact failure the Release arm exists to prevent. Re-installing the same
/// arm stays idempotent.
fn install(paths: AppPaths) -> io::Result<&'static AppPaths> {
    let want_release = paths.is_release();
    let installed = APP_PATHS.get_or_init(|| paths);
    if installed.is_release() != want_release {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "app paths were already initialized as the {} layout; cannot install the {} layout",
                arm_name(installed.is_release()),
                arm_name(want_release),
            ),
        ));
    }
    configure_eseqlisp_roots(installed);
    Ok(installed)
}

fn arm_name(is_release: bool) -> &'static str {
    if is_release { "release" } else { "dev" }
}

/// Detect and install the process layout. Packaged executables use only
/// bundle/user roots; all other executables preserve the workspace behavior.
pub fn init() -> io::Result<&'static AppPaths> {
    let executable = std::env::current_exe()?;
    let paths = match release_paths_for_executable(&executable)? {
        Some(paths) => paths,
        None => AppPaths::dev_from_env(
            crate::paths::sequencer_dir()?,
            crate::paths::workspace_root(),
        )?,
    };
    install(paths)
}

/// Install the release layout for a known bundled executable. This is useful
/// to launch/test a bundle path explicitly and rejects non-bundle paths.
pub fn init_release(executable: &Path) -> io::Result<&'static AppPaths> {
    let paths = release_paths_for_executable(executable)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "release executable is not inside *.app/Contents/MacOS: {}",
                executable.display()
            ),
        )
    })?;
    install(paths)
}

/// Install the dev-layout `AppPaths` for this process. Called at startup next
/// to `paths::enter_sequencer_dir()`, while the workspace is still locatable.
/// Idempotent; later calls keep the first installation.
pub fn init_dev() -> io::Result<()> {
    let paths = AppPaths::dev_from_env(
        crate::paths::sequencer_dir()?,
        crate::paths::workspace_root(),
    )?;
    install(paths)?;
    Ok(())
}

/// Hand eseqlisp the roots it cannot derive itself: the defmacro library and
/// the relative-`(load …)` fallback roots. User scratch buffers load factory
/// scripts with paths like `scripts/sequencers/x.lisp` (or the migrated
/// `content/scripts/…` form); those resolved against the crate-dir cwd before
/// the content/ split and must now fall back to the factory content root.
fn configure_eseqlisp_roots(paths: &AppPaths) {
    eseqlisp::defmacro_library::set_default_library_root(paths.defmacros_dir());
    let factory_root = paths.factory_root();
    let mut roots = vec![factory_root.clone()];
    if let Some(parent) = factory_root.parent() {
        roots.push(parent.to_path_buf());
    }
    eseqlisp::hot_reload::set_global_load_fallback_roots(roots);
}

/// Process-wide accessor. Startup should call [`init`]; tests and helper tools
/// retain a dev fallback so existing library-only entry points remain usable.
pub fn app_paths() -> &'static AppPaths {
    APP_PATHS.get_or_init(|| {
        let sequencer_dir = crate::paths::sequencer_dir()
            .unwrap_or_else(|_| PathBuf::from(env!("ESEQ_DEV_MANIFEST_DIR")));
        let paths = AppPaths::dev_from_env(sequencer_dir, crate::paths::workspace_root())
            .expect("resolve user Lisp root from HOME or ESEQ_CONFIG_DIR");
        configure_eseqlisp_roots(&paths);
        paths
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finder launches a bundle with the working directory set to `/`, so a
    /// relative crash-log path aborts startup with `ReadOnlyFilesystem` before
    /// a window opens. Release must resolve it absolutely.
    #[test]
    fn release_crash_log_is_absolute_and_inside_user_data_root() {
        let paths = release_paths_for_executable_and_home(
            Path::new("/Applications/ESeq.app/Contents/MacOS/metal_seq"),
            Path::new("/Users/test"),
        )
        .expect("bundle path must select Release");
        let crash_log = paths.crash_log_path();
        assert_eq!(
            crash_log,
            paths.user_data_root().join(crate::crash::CRASH_LOG_FILENAME)
        );
        assert!(crash_log.is_absolute(), "{}", crash_log.display());
    }

    #[test]
    fn bundled_executable_selects_release_and_keeps_all_roots_out_of_checkout() {
        let executable = Path::new("/Applications/ESeq.app/Contents/MacOS/metal_seq");
        let paths = release_paths_for_executable_and_home(executable, Path::new("/Users/test"))
            .expect("bundle path must select Release");
        assert!(paths.is_release());
        assert_eq!(
            runtime_layout(executable),
            RuntimeLayout::Release {
                executable_dir: PathBuf::from("/Applications/ESeq.app/Contents/MacOS"),
                contents_resources: PathBuf::from("/Applications/ESeq.app/Contents/Resources"),
            }
        );

        let contents = Path::new("/Applications/ESeq.app/Contents");
        let support =
            Path::new("/Users/test/Library/Application Support/com.universalsequences.eseq");
        let caches = Path::new("/Users/test/Library/Caches/com.universalsequences.eseq");
        let user_lisp = Path::new("/Users/test/.eseq.d");
        let rooted_paths = [
            paths.dgenlisp_tool(),
            paths.dgen_toolchain_root(),
            paths.dgen_abi_dir(),
            paths.perf_probe_projects_dir(),
            paths.ui_monospace_font(),
            paths.factory_root(),
            paths.user_data_root(),
            paths.project_script_root(),
            paths.project_scratch_source_path(),
            paths.preferences_path(),
            paths.local_modules_dir(),
            paths.packages_dir(),
            paths.factory_packages_dir(),
            paths.core_dir(),
            paths.ui_dir(),
            paths.defmacros_dir(),
            paths.midi_fx_dir(),
            paths.presets_dir(),
            paths.rack_presets_dir(),
            paths.kits_dir(),
            paths.processes_dir(),
            paths.scripts_dir(),
            paths.impulses_dir(),
            paths.filter_tables_dir(),
            paths.projects_dir(),
            paths.recordings_dir(),
            paths.samples_dir(),
            paths.sample_db_path(),
            paths.sample_facts_path(),
            paths.sounds_dir(),
            paths.sample_assets_dir(),
            paths.user_filter_tables_dir(),
            paths.user_presets_dir(),
            paths.user_rack_presets_dir(),
            paths.user_kits_dir(),
            paths.effects_dir(),
            paths.instruments_dir(),
            paths.user_effects_dir(),
            paths.user_instruments_dir(),
            paths.factory_assets_dir(),
            paths.user_assets_dir(),
            paths.dgen_asset_fallback_base(),
            paths.dgen_cache_root(),
            paths.convolution_ir_cache_dir(),
            paths.learn_jobs_dir(),
            paths.dgen_scratch_dir(),
            paths.ir_prep_dir(),
        ];
        for path in rooted_paths {
            assert!(
                path.starts_with(contents)
                    || path.starts_with(support)
                    || path.starts_with(caches)
                    || path.starts_with(user_lisp),
                "release path escaped installed/user roots: {}",
                path.display()
            );
            assert!(!path.starts_with("/ws"));
        }
        assert_eq!(paths.user_lisp_root(), user_lisp);
        for path in paths.effect_dirs().into_iter().chain(paths.instrument_dirs()) {
            assert!(path.starts_with(contents) || path.starts_with(support));
        }
    }

    #[test]
    fn near_miss_bundle_paths_stay_on_dev() {
        // Only an exact `*.app/Contents/MacOS/<exe>` shape is a bundle; a
        // checkout that merely contains one of those component names is not.
        for executable in [
            "/ws/MacOS/metal_seq",
            "/ws/Contents/MacOS/metal_seq",
            "/ws/ESeq.app/MacOS/metal_seq",
            "/ws/ESeq.app/Contents/metal_seq",
            "/ws/ESeq/Contents/MacOS/metal_seq",
            "/ws/ESeq.app/Contents/MacOS/nested/metal_seq",
            "/ws/ESeq.app.bak/Contents/MacOS/metal_seq",
            "metal_seq",
        ] {
            assert_eq!(
                runtime_layout(Path::new(executable)),
                RuntimeLayout::Dev,
                "{executable} must not be treated as a bundle"
            );
        }
    }

    #[test]
    fn workspace_executable_selects_dev() {
        let executable = Path::new("/ws/target/debug/metal_seq");
        assert_eq!(runtime_layout(executable), RuntimeLayout::Dev);
        assert!(release_paths_for_executable_and_home(executable, Path::new("/Users/test"))
            .is_none());
    }

    #[test]
    fn release_arm_ignores_dev_toolchain_and_checkout_roots() {
        // Release construction has no override inputs: even values named by
        // ESEQ_DGEN_TOOLCHAIN_ROOT and SEQUENCER_ROOT cannot enter this arm.
        let paths = release_paths_for_executable_and_home(
            Path::new("/Applications/ESeq.app/Contents/MacOS/metal_seq"),
            Path::new("/Users/test"),
        )
        .unwrap();
        let eseq_dgen_toolchain_root = Path::new("/ws/shared-toolchain");
        let sequencer_root = Path::new("/ws/crates/sequencer");
        assert_ne!(paths.dgen_toolchain_root(), eseq_dgen_toolchain_root);
        assert_ne!(paths.factory_root(), sequencer_root);
        assert_eq!(
            paths.dgen_toolchain_root(),
            PathBuf::from("/Applications/ESeq.app/Contents/Resources/dgen-toolchain")
        );
        assert_eq!(
            paths.factory_root(),
            PathBuf::from("/Applications/ESeq.app/Contents/Resources")
        );
    }

    #[test]
    fn dev_paths_resolve_from_captured_roots_only() {
        let paths = AppPaths::dev(
            PathBuf::from("/ws/crates/sequencer"),
            PathBuf::from("/ws"),
            PathBuf::from("/home/test/.eseq.d"),
        );
        assert_eq!(
            paths.dgenlisp_tool(),
            PathBuf::from("/ws/crates/sequencer/tools").join(DGENLISP_TOOL_FILENAME)
        );
        assert_eq!(
            paths.dgen_toolchain_root(),
            PathBuf::from("/ws/crates/sequencer/tools/dgen-toolchain")
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            paths.dgen_abi_dir(),
            PathBuf::from("/ws/crates/sequencer/tools/dgen-toolchain/abi")
        );
        #[cfg(target_os = "linux")]
        assert_eq!(
            paths.dgen_abi_dir(),
            PathBuf::from("/ws/crates/sequencer/tools/toolchain/abi")
        );
        assert_eq!(
            paths.perf_probe_projects_dir(),
            PathBuf::from("/ws/crates/sequencer/tests/fixtures/projects")
        );
        assert_eq!(paths.factory_root(), PathBuf::from("/ws/content"));
        assert_eq!(paths.user_data_root(), PathBuf::from("/ws/.local"));
        assert_eq!(paths.project_script_root(), PathBuf::from("/ws"));
        assert_eq!(
            paths.project_scratch_source_path(),
            PathBuf::from("/ws/.eseqlisp-scratch")
        );
        assert_eq!(
            paths.preferences_path(),
            PathBuf::from("/ws/.eseq/prefs.json")
        );
        assert_eq!(paths.user_lisp_root(), Path::new("/home/test/.eseq.d"));
        assert_eq!(paths.core_dir(), PathBuf::from("/ws/content/core"));
        assert_eq!(paths.ui_dir(), PathBuf::from("/ws/content/ui"));
        assert_eq!(paths.effects_dir(), PathBuf::from("/ws/content/effects"));
        assert_eq!(paths.instruments_dir(), PathBuf::from("/ws/content/instruments"));
        assert_eq!(paths.factory_assets_dir(), PathBuf::from("/ws/content/assets"));
        assert_eq!(paths.user_assets_dir(), PathBuf::from("/ws/.local/assets"));
        assert_eq!(paths.projects_dir(), PathBuf::from("/ws/.local/projects"));
        assert_eq!(paths.sample_db_path(), PathBuf::from("/ws/.local/samples.db"));
        assert_eq!(paths.sample_facts_path(), PathBuf::from("/ws/.local/samples.jsonl"));
        assert_eq!(
            paths.effect_dirs(),
            vec![PathBuf::from("/ws/content/effects"), PathBuf::from("/ws/.local/effects")]
        );
        assert_eq!(
            paths.instrument_dirs(),
            vec![PathBuf::from("/ws/content/instruments"), PathBuf::from("/ws/.local/instruments")]
        );
        assert_eq!(
            paths.dgen_asset_fallback_base(),
            PathBuf::from("/ws/content")
        );
        assert_eq!(
            paths.dgen_cache_root(),
            PathBuf::from("/ws/.eseq/dgenlisp-cache")
        );
        assert_eq!(
            paths.convolution_ir_cache_dir(),
            PathBuf::from("/ws/.eseq/dgenlisp-cache/ir-prep")
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
    fn factory_basic_shapes_asset_has_declared_tensor_shape() {
        let path = app_paths()
            .factory_assets_dir()
            .join("wavetables/basic-shapes.json");
        let value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&path).unwrap_or_else(|error| {
                panic!("read factory asset {}: {error}", path.display())
            }),
        )
        .expect("parse factory basic-shapes asset");
        assert_eq!(value["shape"], serde_json::json!([512, 4]));
        assert_eq!(value["data"].as_array().map(Vec::len), Some(512 * 4));
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
    fn content_addressed_sample_refs_resolve_only_against_the_sample_store() {
        let paths = AppPaths::release(
            PathBuf::from("/App/Contents/MacOS"),
            PathBuf::from("/App/Contents/Resources"),
            PathBuf::from("/configured/user-data"),
            PathBuf::from("/Caches"),
            PathBuf::from("/home/test/.eseq.d"),
        );

        assert_eq!(
            paths.resolve_sample_ref(Path::new("samples/abc123.wav")),
            PathBuf::from("/configured/user-data/samples/abc123.wav")
        );
        assert_eq!(
            paths.resolve_sample_ref(Path::new("fixtures/abc123.wav")),
            PathBuf::from("fixtures/abc123.wav")
        );
        assert_eq!(
            paths.resolve_sample_ref(Path::new("/external/abc123.wav")),
            PathBuf::from("/external/abc123.wav")
        );
    }

    #[test]
    fn release_content_and_user_data_resolve_to_separate_tiers() {
        let paths = AppPaths::release(
            PathBuf::from("/App/Contents/MacOS"),
            PathBuf::from("/App/Contents/Resources"),
            PathBuf::from("/Support"),
            PathBuf::from("/Caches"),
            PathBuf::from("/home/test/.eseq.d"),
        );
        assert_eq!(
            paths.dgenlisp_tool(),
            PathBuf::from("/App/Contents/MacOS").join(DGENLISP_TOOL_FILENAME)
        );
        assert_eq!(
            paths.perf_probe_projects_dir(),
            PathBuf::from("/App/Contents/Resources/test-fixtures/projects")
        );
        assert_eq!(paths.core_dir(), PathBuf::from("/App/Contents/Resources/core"));
        assert_eq!(paths.ui_dir(), PathBuf::from("/App/Contents/Resources/ui"));
        assert_eq!(
            paths.instruments_dir(),
            PathBuf::from("/App/Contents/Resources/instruments")
        );
        assert_eq!(
            paths.user_instruments_dir(),
            PathBuf::from("/Support/instruments")
        );
        assert_eq!(
            paths.factory_assets_dir(),
            PathBuf::from("/App/Contents/Resources/assets")
        );
        assert_eq!(paths.user_assets_dir(), PathBuf::from("/Support/assets"));
        assert_eq!(paths.projects_dir(), PathBuf::from("/Support/projects"));
        assert_eq!(paths.samples_dir(), PathBuf::from("/Support/samples"));
        assert_eq!(paths.sample_db_path(), PathBuf::from("/Support/samples.db"));
        assert_eq!(paths.project_script_root(), PathBuf::from("/Support"));
        assert_eq!(
            paths.project_scratch_source_path(),
            PathBuf::from("/Support/.eseqlisp-scratch")
        );
        assert_eq!(
            paths.preferences_path(),
            PathBuf::from("/Support/prefs.json")
        );
        assert_eq!(
            paths.convolution_ir_cache_dir(),
            PathBuf::from("/Caches/dgen/ir-prep")
        );
        assert_eq!(paths.sample_facts_path(), PathBuf::from("/Support/samples.jsonl"));
        assert_eq!(paths.user_lisp_root(), Path::new("/home/test/.eseq.d"));
    }

    #[test]
    fn user_tier_initialization_and_load_path_are_complete_and_ordered() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("eseq-user-tier-{unique}"));
        let workspace = root.join("workspace");
        let config = root.join("config");
        let paths = AppPaths::dev(
            workspace.join("crates/sequencer"),
            workspace.clone(),
            config.clone(),
        );

        paths.ensure_user_tier().expect("initialize user tier");
        for directory in [
            paths.projects_dir(),
            paths.recordings_dir(),
            paths.samples_dir(),
            paths.sounds_dir(),
            paths.user_instruments_dir(),
            paths.user_effects_dir(),
            paths.user_assets_dir(),
            paths.packages_dir(),
            paths.local_modules_dir(),
        ] {
            assert!(directory.is_dir(), "missing {}", directory.display());
        }
        assert!(!config.join("init.lisp").exists());

        for (dir, identity, module) in [
            ("zeta", "dev/zeta", "dev.zeta.main"),
            ("alpha", "dev/alpha", "dev.alpha.main"),
        ] {
            let package = paths.packages_dir().join(dir);
            std::fs::create_dir_all(package.join("src")).unwrap();
            std::fs::write(package.join("src/main.lisp"), format!("(module {module})")).unwrap();
            std::fs::write(package.join("manifest.json"), format!(
                r#"{{"name":"{identity}","version":"1","entry":"{module}"}}"#
            )).unwrap();
        }
        std::fs::create_dir_all(paths.packages_dir().join(".ignored-no-src")).unwrap();
        // An invalid clone must be reported and skipped, never abort boot by
        // failing load-path construction (same contract as a failed user init).
        std::fs::create_dir_all(paths.packages_dir().join("broken")).unwrap();
        std::fs::write(paths.packages_dir().join("broken/manifest.json"), "not json").unwrap();
        let factory_package = workspace.join("content/packages/dev.factory");
        std::fs::create_dir_all(factory_package.join("src")).unwrap();
        std::fs::write(
            factory_package.join("src/main.lisp"),
            "(module dev.factory.main)",
        )
        .unwrap();
        std::fs::write(
            factory_package.join("manifest.json"),
            r#"{"name":"dev/factory","version":"1","entry":"dev.factory.main"}"#,
        )
        .unwrap();
        assert_eq!(
            paths.load_path().unwrap(),
            vec![
                config.join("packages/local"),
                config.join("packages/alpha/src"),
                config.join("packages/zeta/src"),
                workspace.join("content/packages/dev.factory/src"),
                workspace.join("content"),
            ]
        );
        let (_, errors) = paths.module_load_roots();
        assert_eq!(errors.len(), 1, "the broken package is reported exactly once");
        assert!(errors[0].contains("broken"), "error names the offending package: {}", errors[0]);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn release_user_tier_initialization_uses_application_support() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("eseq-release-user-tier-{unique}"));
        let support = root.join("Library/Application Support/com.universalsequences.eseq");
        let config = root.join("home/.eseq.d");
        let paths = AppPaths::release(
            root.join("ESeq.app/Contents/MacOS"),
            root.join("ESeq.app/Contents/Resources"),
            support.clone(),
            root.join("Library/Caches/com.universalsequences.eseq"),
            config.clone(),
        );

        paths.ensure_user_tier().expect("initialize release user tier");
        assert!(support.is_dir());
        assert!(support.join("projects").is_dir());
        assert!(support.join("instruments").is_dir());
        assert!(config.join("packages").is_dir());
        assert!(config.join("packages/local").is_dir());
        assert!(!config.join("init.lisp").exists());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn user_lisp_root_honors_override_without_reading_or_writing_real_home() {
        assert_eq!(
            resolve_user_lisp_root(
                Some(std::ffi::OsString::from("/tmp/eseq-config")),
                Some(std::ffi::OsString::from("/real-home")),
            )
            .unwrap(),
            PathBuf::from("/tmp/eseq-config")
        );
        assert_eq!(
            resolve_user_lisp_root(None, Some(std::ffi::OsString::from("/home/test"))).unwrap(),
            PathBuf::from("/home/test/.eseq.d")
        );
        assert!(resolve_user_lisp_root(None, None).is_err());
    }

    #[test]
    fn dgenlisp_override_redirects_tool() {
        let mut paths = AppPaths::dev(
            PathBuf::from("/ws/crates/sequencer"),
            PathBuf::from("/ws"),
            PathBuf::from("/home/test/.eseq.d"),
        );
        let AppPaths::Dev {
            dgenlisp_tool_override,
            ..
        } = &mut paths
        else {
            unreachable!()
        };
        *dgenlisp_tool_override = Some(PathBuf::from("/custom/DGenLisp"));
        assert_eq!(paths.dgenlisp_tool(), PathBuf::from("/custom/DGenLisp"));
    }

    #[test]
    fn toolchain_override_redirects_root_and_preflight_reports_it() {
        let mut paths = AppPaths::dev(
            PathBuf::from("/ws/crates/sequencer"),
            PathBuf::from("/ws"),
            PathBuf::from("/home/test/.eseq.d"),
        );
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
    fn missing_default_stage_is_a_hard_error_on_every_host() {
        let paths = AppPaths::dev(
            PathBuf::from("/nonexistent/crates/sequencer"),
            PathBuf::from("/nonexistent"),
            PathBuf::from("/home/test/.eseq.d"),
        );
        let err = paths
            .dgen_toolchain_root_checked()
            .expect_err("missing stage must be a hard error");
        assert!(err.contains("rebuild_dgenlisp_tool.sh"), "{err}");
        assert!(err.contains("tools/dgen-toolchain"), "{err}");
    }

    #[test]
    fn wrong_target_toolchain_stage_is_a_hard_error() {
        let root = std::env::temp_dir().join(format!(
            "eseq-wrong-dgen-toolchain-target-{}",
            std::process::id()
        ));
        let stage = root.join("dgen-toolchain");
        std::fs::create_dir_all(&stage).expect("create test stage");
        let wrong_target = if DGEN_TOOLCHAIN_TARGET == "arm64-apple-macos" {
            "x86_64-unknown-linux-gnu"
        } else {
            "arm64-apple-macos"
        };
        std::fs::write(
            stage.join("VERSION.json"),
            format!(r#"{{"target":"{wrong_target}"}}"#),
        )
        .expect("write stage identity");

        let mut paths = AppPaths::dev(
            PathBuf::from("/ws/crates/sequencer"),
            PathBuf::from("/ws"),
            PathBuf::from("/home/test/.eseq.d"),
        );
        let AppPaths::Dev {
            dgen_toolchain_override,
            ..
        } = &mut paths
        else {
            unreachable!()
        };
        *dgen_toolchain_override = Some(stage);
        let err = paths
            .dgen_toolchain_root_checked()
            .expect_err("wrong-target stage must be rejected before compiler invocation");
        assert!(err.contains(wrong_target), "{err}");
        assert!(err.contains(DGEN_TOOLCHAIN_TARGET), "{err}");
        assert!(err.contains("target-specific"), "{err}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_dgenlisp_tool_error_mentions_fetch_script() {
        let paths = AppPaths::dev(
            PathBuf::from("/nonexistent/crates/sequencer"),
            PathBuf::from("/nonexistent"),
            PathBuf::from("/home/test/.eseq.d"),
        );
        let err = paths
            .dgenlisp_tool_checked()
            .expect_err("missing compiler must be a hard error");
        assert!(err.contains("scripts/fetch_dgenlisp.sh"), "{err}");
        assert!(err.contains(DGENLISP_TOOL_FILENAME), "{err}");

        let mut overridden = AppPaths::dev(
            PathBuf::from("/nonexistent/crates/sequencer"),
            PathBuf::from("/nonexistent"),
            PathBuf::from("/home/test/.eseq.d"),
        );
        let AppPaths::Dev {
            dgenlisp_tool_override,
            ..
        } = &mut overridden
        else {
            unreachable!()
        };
        *dgenlisp_tool_override = Some(PathBuf::from("/custom/DGenLisp"));
        let err = overridden
            .dgenlisp_tool_checked()
            .expect_err("missing override compiler must be a hard error");
        assert!(err.contains("/custom/DGenLisp"), "{err}");
        assert!(err.contains("ESEQ_DGENLISP_TOOL"), "{err}");
    }

    #[test]
    fn global_accessor_matches_workspace_layout() {
        let paths = app_paths();
        let sequencer_dir = crate::paths::sequencer_dir().expect("locate sequencer dir");
        assert_eq!(
            paths.dgenlisp_tool(),
            sequencer_dir.join("tools").join(DGENLISP_TOOL_FILENAME)
        );
        assert_eq!(
            paths.dgen_cache_root(),
            crate::paths::workspace_root()
                .join(".eseq")
                .join("dgenlisp-cache")
        );
    }
}
