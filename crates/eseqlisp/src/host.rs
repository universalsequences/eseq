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
    AuthoringTransactionBegin {
        id: u64,
        label: String,
    },
    AuthoringTransactionEnd {
        id: u64,
        success: bool,
    },
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

/// Tone of a window-level toast. Success toasts time out quickly; error
/// toasts linger and also clear on the next keypress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Error,
}

impl ToastKind {
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "success" | "ok" => Some(Self::Success),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
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
