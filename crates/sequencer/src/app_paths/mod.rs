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
        }
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
            AppPaths::Dev { sequencer_dir, .. } => sequencer_dir.join("tools/dgen-toolchain"),
            AppPaths::Release {
                contents_resources, ..
            } => contents_resources.join("dgen-toolchain"),
        }
    }

    /// ABI allowlist dir (`exports-v1.txt`, `libsystem-symbols-v1.txt`) read
    /// by the Rust binary audit (slice E5).
    pub fn dgen_abi_dir(&self) -> PathBuf {
        self.dgen_toolchain_root().join("abi")
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

static APP_PATHS: OnceLock<AppPaths> = OnceLock::new();

/// Install the dev-layout `AppPaths` for this process. Called at startup next
/// to `paths::enter_sequencer_dir()`, while the workspace is still locatable.
/// Idempotent; later calls keep the first installation.
pub fn init_dev() -> io::Result<()> {
    let paths = AppPaths::dev(
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
        AppPaths::dev(sequencer_dir, crate::paths::workspace_root())
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
            paths.dgen_scratch_dir(),
            std::env::temp_dir().join("sequencer_dgenlisp")
        );
        assert_eq!(
            paths.ir_prep_dir(),
            std::env::temp_dir().join("sequencer_ir_prep")
        );
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
