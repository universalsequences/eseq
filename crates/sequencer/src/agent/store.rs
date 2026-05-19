use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::SystemTime;

use super::providers::{AgentProviderKind, AgentProviderState};
use serde::{Deserialize, Serialize};

pub type ConvId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    General,
    Instrument,
    Effect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Streaming,
    Compiling,
    Auditioning,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub text: String,
    pub reasoning_content: Option<String>,
    pub ts: SystemTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InstrumentDraft {
    pub dsp_source: String,
    pub ui_source: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectDraft {
    pub name: String,
    pub dsp_source: String,
    pub ui_source: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DraftSlot {
    pub conv_id: ConvId,
    pub kind: AgentKind,
    pub instrument: Option<InstrumentDraft>,
    pub draft: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedInstrumentTarget {
    pub track_index: usize,
    pub instrument_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedEffectTarget {
    pub track_index: usize,
    pub slot_index: usize,
    pub effect_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditionResult {
    pub ran: bool,
    pub peak_db: f32,
    pub rms_db: f32,
    pub clipped: bool,
    pub duration_ms: u32,
    pub silent: bool,
    pub differs_from_input: Option<bool>,
    pub diff_rms_db: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct ConversationState {
    pub id: ConvId,
    pub kind: AgentKind,
    pub status: AgentStatus,
    pub messages: Vec<Message>,
    pub draft: Option<InstrumentDraft>,
    pub effect_draft: Option<EffectDraft>,
    pub draft_handle: Option<DraftSlot>,
    pub stub_instrument_target: Option<AcceptedInstrumentTarget>,
    pub accepted_instrument_target: Option<AcceptedInstrumentTarget>,
    pub accepted_effect_target: Option<AcceptedEffectTarget>,
    pub effect_draft_applied: bool,
    pub finalized_instrument_name: Option<String>,
    pub finalized_effect_name: Option<String>,
    pub last_compile_error: Option<String>,
    pub last_audition: Option<AuditionResult>,
    pub retries_this_turn: u8,
    pub generation: u64,
    pub created_at: SystemTime,
    pub provider: AgentProviderKind,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct ConversationSnapshot {
    pub state: ConversationState,
}

pub(crate) struct RunningTask {
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) handle: JoinHandle<()>,
}

#[derive(Clone)]
pub struct ConversationStore {
    inner: Arc<Mutex<HashMap<ConvId, ConversationState>>>,
    task_handles: Arc<Mutex<HashMap<ConvId, RunningTask>>>,
    next_id: Arc<AtomicU64>,
    sample_rate: u32,
}

impl ConversationStore {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            task_handles: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            sample_rate,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub(crate) fn inner(&self) -> Arc<Mutex<HashMap<ConvId, ConversationState>>> {
        Arc::clone(&self.inner)
    }

    pub(crate) fn task_handles(&self) -> Arc<Mutex<HashMap<ConvId, RunningTask>>> {
        Arc::clone(&self.task_handles)
    }

    pub fn new_conversation(&self, kind: AgentKind) -> ConvId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let provider_state = AgentProviderState::from_env();
        let provider = provider_state.selected_provider;
        let model = provider_state
            .selected_model()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "gpt-5.5".to_string());
        let mut inner = self.inner.lock().unwrap();
        inner.insert(
            id,
            ConversationState {
                id,
                kind,
                status: AgentStatus::Idle,
                messages: Vec::new(),
                draft: None,
                effect_draft: None,
                draft_handle: None,
                stub_instrument_target: None,
                accepted_instrument_target: None,
                accepted_effect_target: None,
                effect_draft_applied: false,
                finalized_instrument_name: None,
                finalized_effect_name: None,
                last_compile_error: None,
                last_audition: None,
                retries_this_turn: 0,
                generation: 1,
                created_at: SystemTime::now(),
                provider,
                model,
            },
        );
        id
    }

    pub fn list(&self) -> Vec<ConvId> {
        let mut ids = self
            .inner
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }

    pub fn snapshot(&self, id: ConvId) -> Option<ConversationSnapshot> {
        self.inner
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .map(|state| ConversationSnapshot { state })
    }

    pub fn set_model(
        &self,
        id: ConvId,
        provider: AgentProviderKind,
        model: impl Into<String>,
    ) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .get_mut(&id)
            .ok_or_else(|| format!("unknown agent conversation {id}"))?;
        state.provider = provider;
        state.model = model.into();
        bump(state);
        Ok(())
    }

    pub fn discard(&self, id: ConvId) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .get_mut(&id)
            .ok_or_else(|| format!("unknown agent conversation {id}"))?;
        state.draft = None;
        state.effect_draft = None;
        state.draft_handle = None;
        state.effect_draft_applied = false;
        state.last_compile_error = None;
        state.last_audition = None;
        bump(state);
        Ok(())
    }

    pub fn push_system_message(&self, id: ConvId, text: impl Into<String>) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .get_mut(&id)
            .ok_or_else(|| format!("unknown agent conversation {id}"))?;
        push_message(state, Role::System, text.into());
        Ok(())
    }

    pub fn push_tool_message(&self, id: ConvId, text: impl Into<String>) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .get_mut(&id)
            .ok_or_else(|| format!("unknown agent conversation {id}"))?;
        push_message(state, Role::Tool, text.into());
        Ok(())
    }

    pub fn set_accepted_instrument_target(
        &self,
        id: ConvId,
        target: AcceptedInstrumentTarget,
    ) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .get_mut(&id)
            .ok_or_else(|| format!("unknown agent conversation {id}"))?;
        state.accepted_instrument_target = Some(target);
        state.stub_instrument_target = None;
        state.finalized_instrument_name = None;
        bump(state);
        Ok(())
    }

    pub fn set_stub_instrument_target(
        &self,
        id: ConvId,
        target: AcceptedInstrumentTarget,
    ) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .get_mut(&id)
            .ok_or_else(|| format!("unknown agent conversation {id}"))?;
        state.stub_instrument_target = Some(target);
        bump(state);
        Ok(())
    }

    pub fn set_accepted_effect_target(
        &self,
        id: ConvId,
        target: AcceptedEffectTarget,
    ) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .get_mut(&id)
            .ok_or_else(|| format!("unknown agent conversation {id}"))?;
        state.accepted_effect_target = Some(target);
        state.effect_draft_applied = true;
        state.finalized_effect_name = None;
        bump(state);
        Ok(())
    }

    pub fn set_finalized_instrument_name(
        &self,
        id: ConvId,
        instrument_name: impl Into<String>,
    ) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .get_mut(&id)
            .ok_or_else(|| format!("unknown agent conversation {id}"))?;
        state.draft = None;
        state.finalized_instrument_name = Some(instrument_name.into());
        bump(state);
        Ok(())
    }

    pub fn set_finalized_effect_name(
        &self,
        id: ConvId,
        effect_name: impl Into<String>,
    ) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .get_mut(&id)
            .ok_or_else(|| format!("unknown agent conversation {id}"))?;
        state.effect_draft = None;
        state.effect_draft_applied = false;
        state.finalized_effect_name = Some(effect_name.into());
        bump(state);
        Ok(())
    }

    pub fn close(&self, id: ConvId) {
        self.cancel(id).ok();
        self.inner.lock().unwrap().remove(&id);
    }

    pub fn cancel(&self, id: ConvId) -> Result<(), String> {
        if let Some(task) = self.task_handles.lock().unwrap().remove(&id) {
            task.cancel.store(true, Ordering::Release);
            let _ = task.handle.thread().id();
        }
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .get_mut(&id)
            .ok_or_else(|| format!("unknown agent conversation {id}"))?;
        state.status = AgentStatus::Cancelled;
        push_message(state, Role::System, "request cancelled".to_string());
        Ok(())
    }
}

pub(crate) fn bump(state: &mut ConversationState) {
    state.generation = state.generation.saturating_add(1);
}

pub(crate) fn push_message(state: &mut ConversationState, role: Role, text: String) {
    push_message_with_reasoning(state, role, text, None);
}

pub(crate) fn push_message_with_reasoning(
    state: &mut ConversationState,
    role: Role,
    text: String,
    reasoning_content: Option<String>,
) {
    state.messages.push(Message {
        role,
        text,
        reasoning_content,
        ts: SystemTime::now(),
    });
    bump(state);
}

#[cfg(test)]
mod tests {
    use super::{AgentKind, AgentStatus, ConversationStore};

    #[test]
    fn creates_and_lists_conversations() {
        let store = ConversationStore::new(44_100);
        let id = store.new_conversation(AgentKind::Instrument);
        assert_eq!(store.list(), vec![id]);
        let snapshot = store.snapshot(id).unwrap();
        assert_eq!(snapshot.state.status, AgentStatus::Idle);
        assert_eq!(snapshot.state.generation, 1);
    }

    #[test]
    fn set_model_bumps_generation() {
        let store = ConversationStore::new(44_100);
        let id = store.new_conversation(AgentKind::Instrument);
        let before = store.snapshot(id).unwrap().state.generation;
        store
            .set_model(
                id,
                crate::agent::providers::AgentProviderKind::Gemini,
                "gemini-2.5-pro",
            )
            .unwrap();
        let after = store.snapshot(id).unwrap().state.generation;
        assert!(after > before);
    }

    #[test]
    fn finalized_effect_consumes_draft_state() {
        let store = ConversationStore::new(44_100);
        let id = store.new_conversation(AgentKind::Effect);
        {
            let inner = store.inner();
            let mut inner = inner.lock().unwrap();
            let state = inner.get_mut(&id).unwrap();
            state.effect_draft = Some(super::EffectDraft {
                name: "draft".to_string(),
                dsp_source: "(def in_l (in 1 @name left))".to_string(),
                ui_source: "(defeffect-ui (label \"draft\"))".to_string(),
            });
            state.effect_draft_applied = false;
        }

        store
            .set_finalized_effect_name(id, "saved-effect/")
            .expect("finalize effect");

        let snapshot = store.snapshot(id).unwrap();
        assert_eq!(
            snapshot.state.finalized_effect_name.as_deref(),
            Some("saved-effect/")
        );
        assert!(snapshot.state.effect_draft.is_none());
        assert!(!snapshot.state.effect_draft_applied);
    }

    #[test]
    fn finalized_instrument_consumes_draft_state() {
        let store = ConversationStore::new(44_100);
        let id = store.new_conversation(AgentKind::Instrument);
        {
            let inner = store.inner();
            let mut inner = inner.lock().unwrap();
            let state = inner.get_mut(&id).unwrap();
            state.draft = Some(super::InstrumentDraft {
                dsp_source: "(out 0 1 @name audio)".to_string(),
                ui_source: "(defsynth-ui (label \"draft\"))".to_string(),
            });
        }

        store
            .set_finalized_instrument_name(id, "saved-instrument/")
            .expect("finalize instrument");

        let snapshot = store.snapshot(id).unwrap();
        assert_eq!(
            snapshot.state.finalized_instrument_name.as_deref(),
            Some("saved-instrument/")
        );
        assert!(snapshot.state.draft.is_none());
    }
}
