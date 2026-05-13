/// Extract the last fenced dgenlisp code block from assistant text.
///
/// Agent Mode treats generated source as visible assistant text, so the post-turn
/// pipeline must be strict about only consuming fenced `dgenlisp` blocks.
pub fn last_dgenlisp_block(text: &str) -> Option<String> {
    last_fenced_block(text, |lang| lang == "dgenlisp")
}

pub fn last_eseqlisp_block(text: &str) -> Option<String> {
    last_fenced_block(text, |lang| lang == "eseqlisp" || lang == "lisp")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentArtifacts {
    pub dsp_source: String,
    pub ui_source: String,
}

pub fn instrument_artifacts(text: &str) -> Result<InstrumentArtifacts, String> {
    let dsp_source = last_dgenlisp_block(text).ok_or_else(|| {
        "response must include a fenced ```dgenlisp block containing dsp.lisp".to_string()
    })?;
    let ui_source = last_eseqlisp_block(text).ok_or_else(|| {
        "response must include a fenced ```eseqlisp block containing ui.lisp".to_string()
    })?;
    Ok(InstrumentArtifacts {
        dsp_source,
        ui_source,
    })
}

fn last_fenced_block(text: &str, mut accepts_lang: impl FnMut(&str) -> bool) -> Option<String> {
    let mut last = None;
    let mut rest = text;

    while let Some(open_idx) = rest.find("```") {
        rest = &rest[open_idx + 3..];
        let lang_end = rest.find('\n')?;
        let lang = rest[..lang_end].trim().to_ascii_lowercase();
        rest = &rest[lang_end + 1..];
        let close_idx = rest.find("```")?;
        let body = &rest[..close_idx];
        if accepts_lang(&lang) {
            last = Some(body.trim_matches('\n').to_string());
        }
        rest = &rest[close_idx + 3..];
    }

    last
}

#[cfg(test)]
mod tests {
    use super::{instrument_artifacts, last_dgenlisp_block, last_eseqlisp_block};

    #[test]
    fn extracts_last_dgenlisp_block() {
        let text = "one\n```dgenlisp\n(a)\n```\ntwo\n```dgenlisp\n(b)\n```";
        assert_eq!(last_dgenlisp_block(text).as_deref(), Some("(b)"));
    }

    #[test]
    fn ignores_other_fences() {
        let text = "```rust\nfn main() {}\n```";
        assert_eq!(last_dgenlisp_block(text), None);
    }

    #[test]
    fn requires_closed_fence() {
        let text = "```dgenlisp\n(a)";
        assert_eq!(last_dgenlisp_block(text), None);
    }

    #[test]
    fn extracts_required_instrument_artifacts() {
        let text = "ok\n```dgenlisp\n(out x 1)\n```\n```eseqlisp\n(defsynth-ui (ui-param-control \"gain\"))\n```";
        let artifacts = instrument_artifacts(text).unwrap();
        assert_eq!(artifacts.dsp_source, "(out x 1)");
        assert_eq!(
            artifacts.ui_source,
            "(defsynth-ui (ui-param-control \"gain\"))"
        );
    }

    #[test]
    fn accepts_lisp_as_ui_fence_alias() {
        let text = "```lisp\n(defsynth-ui (label \"x\"))\n```";
        assert_eq!(
            last_eseqlisp_block(text).as_deref(),
            Some("(defsynth-ui (label \"x\"))")
        );
    }
}
