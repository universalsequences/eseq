//! The one validate → compile → validate-panel → audition pipeline an authored
//! instrument or effect must pass. In-app Agent Mode and the `eseq instrument
//! check` / `eseq effect check` CLI both run it, so an external agent working
//! in the user's content folder gets exactly the verdicts the in-app agent
//! gets.

use std::fmt;
use std::path::Path;

use super::audition::{audition_feedback, audition_loaded_effect, audition_loaded_instrument};
use super::dsp_validate::{validate_effect_dsp_source, validate_instrument_dsp_source};
use super::store::AuditionResult;
use super::ui_validate::{
    check_ui_structure, lint_ui_layout, validate_effect_ui_source, validate_instrument_ui_source,
};
use crate::lisp_host::CompileResult;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactKind {
    Instrument,
    Effect,
}

/// How strictly to hold the sources to Agent Mode's authoring template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyMode {
    /// Agent Mode drafts: the template rules (fixed mod1..mod4 inputs, named
    /// stereo channels, lego-only panels, …) are errors, because its prompt
    /// and retry loop are built around them.
    Agent,
    /// Anything the host can load, factory content included: only what breaks
    /// compiling, the panel's structure, or the audition fails; the template
    /// rules come back as warnings.
    Library,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyStage {
    DspValidation,
    Compile,
    UiValidation,
    Audition,
}

impl VerifyStage {
    pub fn label(self) -> &'static str {
        match self {
            VerifyStage::DspValidation => "dsp validation",
            VerifyStage::Compile => "dsp compile",
            VerifyStage::UiValidation => "ui validation",
            VerifyStage::Audition => "audition",
        }
    }
}

#[derive(Clone, Debug)]
pub struct VerifyFailure {
    pub stage: VerifyStage,
    pub message: String,
}

impl fmt::Display for VerifyFailure {
    /// The wording the in-app agent has always been shown; its prompts and
    /// retry loop key off these prefixes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.stage {
            VerifyStage::DspValidation => write!(f, "dsp.lisp validation error:\n{}", self.message),
            VerifyStage::Compile => write!(f, "compile error:\n{}", self.message),
            VerifyStage::UiValidation => write!(f, "ui.lisp validation error:\n{}", self.message),
            VerifyStage::Audition => f.write_str(&self.message),
        }
    }
}

pub struct VerifyReport {
    pub compile: CompileResult,
    /// False when no panel was supplied (an effect may ship without one).
    pub ui_checked: bool,
    pub audition: AuditionResult,
    pub feedback: String,
    /// Template-rule findings demoted in [`VerifyMode::Library`].
    pub warnings: Vec<String>,
}

/// Run every stage. `ui_source: None` skips panel validation; the host then
/// shows its generated default panel. A silent or clipping render fails at
/// [`VerifyStage::Audition`]; so does an effect whose output matches its input,
/// in [`VerifyMode::Agent`] only.
pub fn verify_sources(
    kind: ArtifactKind,
    mode: VerifyMode,
    dsp_source: &str,
    ui_source: Option<&str>,
    sample_rate: u32,
    asset_base: Option<&Path>,
) -> Result<VerifyReport, VerifyFailure> {
    let fail = |stage, message: String| VerifyFailure { stage, message };
    let mut warnings = Vec::new();

    let dsp_rules = match kind {
        ArtifactKind::Instrument => validate_instrument_dsp_source(dsp_source),
        ArtifactKind::Effect => validate_effect_dsp_source(dsp_source),
    };
    match (dsp_rules, mode) {
        (Ok(()), _) => {}
        (Err(error), VerifyMode::Agent) => return Err(fail(VerifyStage::DspValidation, error)),
        (Err(error), VerifyMode::Library) => warnings.push(error),
    }

    let compile = match kind {
        ArtifactKind::Instrument => crate::lisp_host::compile_and_load_instrument_with_asset_base(
            dsp_source,
            sample_rate,
            asset_base,
        ),
        ArtifactKind::Effect => {
            crate::lisp_host::compile_and_load_with_asset_base(dsp_source, sample_rate, asset_base)
        }
    }
    .map_err(|error| fail(VerifyStage::Compile, error))?;

    if let Some(ui_source) = ui_source {
        match mode {
            VerifyMode::Agent => match kind {
                ArtifactKind::Instrument => {
                    validate_instrument_ui_source(ui_source, &compile.manifest)
                }
                ArtifactKind::Effect => validate_effect_ui_source(ui_source, &compile.manifest),
            }
            .map_err(|error| fail(VerifyStage::UiValidation, error))?,
            VerifyMode::Library => {
                check_ui_structure(ui_source, &compile.manifest, kind)
                    .map_err(|error| fail(VerifyStage::UiValidation, error))?;
                if let Err(error) = lint_ui_layout(ui_source) {
                    warnings.push(error);
                }
            }
        }
    }

    let audition = match kind {
        ArtifactKind::Instrument => audition_loaded_instrument(&compile, sample_rate),
        ArtifactKind::Effect => audition_loaded_effect(&compile, sample_rate),
    }
    .map_err(|error| fail(VerifyStage::Audition, format!("audition failed:\n{error}")))?;
    let feedback = audition_feedback(&audition);
    if audition.silent || audition.clipped {
        return Err(fail(VerifyStage::Audition, feedback));
    }
    if audition.differs_from_input == Some(false) {
        // At default settings a compressor under threshold or a sidechain
        // effect with no key legitimately passes audio through untouched.
        match mode {
            VerifyMode::Agent => return Err(fail(VerifyStage::Audition, feedback)),
            VerifyMode::Library => warnings.push(feedback.clone()),
        }
    }

    Ok(VerifyReport {
        compile,
        ui_checked: ui_source.is_some(),
        audition,
        feedback,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::{verify_sources, ArtifactKind, VerifyMode, VerifyStage};

    const SAMPLE_RATE: u32 = 44_100;

    /// Compiles and changes the signal, but capitalizes its channel names,
    /// which breaks Agent Mode's `@name left` template rule.
    const CAPITALIZED_CHANNEL_EFFECT: &str = r#"
(param drive @min 0.0 @max 1.0 @default 0.5)
(def input_l (in 1 @name Left))
(def input_r (in 2 @name Right))
(out (* input_l drive) 1 @name Left)
(out (* input_r drive) 2 @name Right)
"#;

    #[test]
    fn library_mode_demotes_agent_template_rules_to_warnings() {
        let failure = verify_sources(
            ArtifactKind::Effect,
            VerifyMode::Agent,
            CAPITALIZED_CHANNEL_EFFECT,
            None,
            SAMPLE_RATE,
            None,
        )
        .err()
        .expect("agent mode keeps the channel-name rule");
        assert_eq!(failure.stage, VerifyStage::DspValidation);

        let report = verify_sources(
            ArtifactKind::Effect,
            VerifyMode::Library,
            CAPITALIZED_CHANNEL_EFFECT,
            None,
            SAMPLE_RATE,
            None,
        )
        .unwrap_or_else(|failure| panic!("library mode should pass: {failure}"));
        assert!(
            report.warnings.iter().any(|warning| warning.contains("@name left")),
            "warnings: {:?}",
            report.warnings
        );
    }

    #[test]
    fn library_mode_auditions_mono_effects() {
        let source = r#"
(param drive @min 0.0 @max 1.0 @default 0.5)
(def input (in 1 @name left))
(out (* input drive) 1 @name left)
"#;
        let report = verify_sources(
            ArtifactKind::Effect,
            VerifyMode::Library,
            source,
            None,
            SAMPLE_RATE,
            None,
        )
        .unwrap_or_else(|failure| panic!("mono effect should audition: {failure}"));
        assert!(!report.audition.silent);
    }

    #[test]
    fn passthrough_fails_agent_mode_but_only_warns_in_library_mode() {
        let source = r#"
(param unused @min 0.0 @max 1.0 @default 0.5)
(def input_l (in 1 @name left))
(def input_r (in 2 @name right))
(out input_l 1 @name left)
(out input_r 2 @name right)
"#;
        let failure = verify_sources(
            ArtifactKind::Effect,
            VerifyMode::Agent,
            source,
            None,
            SAMPLE_RATE,
            None,
        )
        .err()
        .expect("agent mode rejects a passthrough draft");
        assert_eq!(failure.stage, VerifyStage::Audition);
        assert!(failure.message.contains("PASSTHROUGH"), "{failure}");

        let report = verify_sources(
            ArtifactKind::Effect,
            VerifyMode::Library,
            source,
            None,
            SAMPLE_RATE,
            None,
        )
        .unwrap_or_else(|failure| panic!("library mode should pass: {failure}"));
        assert!(
            report.warnings.iter().any(|warning| warning.contains("PASSTHROUGH")),
            "warnings: {:?}",
            report.warnings
        );
    }
}
