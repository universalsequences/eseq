use std::path::PathBuf;

use crate::vm::Value;

pub type BufferId = usize;

#[derive(Debug, Clone, PartialEq)]
pub enum CompileKind {
    Instrument,
    Effect,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HostCommand {
    CompileInstrument {
        source: String,
        suggested_name: Option<String>,
        buffer_id: BufferId,
        path: Option<PathBuf>,
    },
    CompileEffect {
        source: String,
        suggested_name: Option<String>,
        buffer_id: BufferId,
        path: Option<PathBuf>,
    },
    Custom {
        name: String,
        payload: Value,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    Status(String),
    Error(String),
    CommandStarted {
        label: String,
    },
    CommandFinished {
        label: String,
        success: bool,
        message: Option<String>,
    },
    CompileFinished {
        kind: CompileKind,
        success: bool,
        name: Option<String>,
        diagnostics: Option<String>,
    },
    BufferSaved {
        buffer_id: BufferId,
        path: PathBuf,
    },
}
