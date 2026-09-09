//! Export failures retain their phase and underlying I/O cause. Worker status
//! carries the phase and a human-readable diagnostic across process boundaries.
use serde::{Deserialize, Serialize};
use std::{fmt, io};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportStage {
    Validation,
    Preparation,
    Rendering,
    Writing,
    Publication,
}

#[derive(Debug)]
pub struct ExportError {
    stage: ExportStage,
    source: io::Error,
}

impl ExportError {
    pub(crate) fn at(stage: ExportStage, source: io::Error) -> Self { Self { stage, source } }
    pub fn stage(&self) -> ExportStage { self.stage }
    pub fn kind(&self) -> io::ErrorKind { self.source.kind() }
    pub fn is_cancelled(&self) -> bool { self.kind() == io::ErrorKind::Interrupted }
    pub(crate) fn validation(source: io::Error) -> Self { Self::at(ExportStage::Validation, source) }
    pub(crate) fn preparation(source: io::Error) -> Self { Self::at(ExportStage::Preparation, source) }
    pub(crate) fn rendering(source: io::Error) -> Self { Self::at(ExportStage::Rendering, source) }
    pub(crate) fn writing(source: io::Error) -> Self { Self::at(ExportStage::Writing, source) }
    pub(crate) fn publication(source: io::Error) -> Self { Self::at(ExportStage::Publication, source) }
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let stage = match self.stage {
            ExportStage::Validation => "validation",
            ExportStage::Preparation => "preparation",
            ExportStage::Rendering => "rendering",
            ExportStage::Writing => "writing",
            ExportStage::Publication => "publication",
        };
        write!(f, "{stage}: {}", self.source)
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> { Some(&self.source) }
}
