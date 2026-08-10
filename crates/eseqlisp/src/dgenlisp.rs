//! Shared access to the generated DGenLisp language manifest.
//!
//! The patcher and DGenLisp text editor are two views of the same language,
//! so they must not maintain independent operator catalogs. Patcher-only graph
//! conveniences are deliberately layered on by the patcher instead of being
//! added to this manifest.

use std::sync::OnceLock;

pub(crate) fn manifest() -> &'static serde_json::Value {
    static MANIFEST: OnceLock<serde_json::Value> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../sequencer/tools/dgenlisp-operators.json"
        ))
        .expect("bundled dgenlisp-operators.json must be valid JSON")
    })
}
