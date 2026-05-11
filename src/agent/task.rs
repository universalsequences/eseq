use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::audition::{audition_feedback, audition_loaded_instrument};
use super::network::AgentNetworkClient;
use super::parse::{instrument_artifacts, last_dgenlisp_block, InstrumentArtifacts};
use super::providers::{AgentMessage, AgentMessageRole};
use super::store::{
    bump, push_message, AgentKind, AgentStatus, ConvId, ConversationState, ConversationStore,
    InstrumentDraft, Role, RunningTask,
};
use super::ui_validate::validate_instrument_ui_source;

const MAX_RETRIES_PER_TURN: u8 = 3;
const MAX_REQUEST_ATTEMPTS: u8 = 3;

impl ConversationStore {
    pub fn send(&self, id: ConvId, prompt: impl Into<String>) -> Result<(), String> {
        let prompt = prompt.into();
        eprintln!(
            "[agent] send requested conv={id} prompt_len={} prompt={:?}",
            prompt.len(),
            prompt
        );
        if prompt.trim().is_empty() {
            eprintln!("[agent] send rejected conv={id}: empty prompt");
            return Err("agent prompt is empty".to_string());
        }

        {
            let handles = self.task_handles();
            if handles.lock().unwrap().contains_key(&id) {
                eprintln!("[agent] send rejected conv={id}: request already in flight");
                return Err("agent request already in flight".to_string());
            }
        }

        {
            let inner = self.inner();
            let mut inner = inner.lock().unwrap();
            let state = inner
                .get_mut(&id)
                .ok_or_else(|| format!("unknown agent conversation {id}"))?;
            eprintln!(
                "[agent] conv={id} transition {:?} -> Streaming",
                state.status
            );
            state.retries_this_turn = 0;
            state.last_compile_error = None;
            state.status = AgentStatus::Streaming;
            push_message(state, Role::User, prompt);
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let task_cancel = Arc::clone(&cancel);
        let store = self.clone();
        let handle = std::thread::spawn(move || {
            eprintln!("[agent] worker started conv={id}");
            run_conversation_turn(store, id, task_cancel);
            eprintln!("[agent] worker exited conv={id}");
        });
        self.task_handles()
            .lock()
            .unwrap()
            .insert(id, RunningTask { cancel, handle });
        Ok(())
    }
}

fn run_conversation_turn(store: ConversationStore, id: ConvId, cancel: Arc<AtomicBool>) {
    loop {
        if cancel.load(Ordering::Acquire) {
            eprintln!("[agent] worker observed cancellation conv={id}");
            return;
        }
        let request = match build_request(&store, id) {
            Ok(request) => request,
            Err(error) => {
                eprintln!("[agent] build request failed conv={id}: {error}");
                set_error(&store, id, error);
                return;
            }
        };
        eprintln!(
            "[agent] request start conv={id} kind={:?} provider={:?} model={} messages={}",
            request.kind,
            request.provider,
            request.model,
            request.messages.len()
        );
        let system_prompt = system_prompt_for(request.kind);
        let response = match execute_text_turn_with_retries(
            request.provider,
            &request.model,
            system_prompt,
            &request.messages,
            id,
            &cancel,
        ) {
            Ok(text) => {
                let has_dsp = last_dgenlisp_block(&text).is_some();
                let has_artifacts = instrument_artifacts(&text).is_ok();
                eprintln!(
                    "[agent] request ok conv={id} response_len={} has_dgenlisp_block={} has_required_artifacts={}",
                    text.len(),
                    has_dsp,
                    has_artifacts
                );
                text
            }
            Err(error) => {
                eprintln!("[agent] request failed conv={id}: {error}");
                set_error(&store, id, format!("request failed: {error}"));
                return;
            }
        };
        if cancel.load(Ordering::Acquire) {
            eprintln!("[agent] dropping response after cancellation conv={id}");
            return;
        }
        {
            let inner = store.inner();
            let mut inner = inner.lock().unwrap();
            let Some(state) = inner.get_mut(&id) else {
                return;
            };
            push_message(state, Role::Assistant, response.clone());
        }

        match run_post_turn_pipeline(&store, id, &response) {
            PipelineOutcome::Done => {
                eprintln!("[agent] turn done conv={id}");
                store.task_handles().lock().unwrap().remove(&id);
                return;
            }
            PipelineOutcome::Retry => {
                eprintln!("[agent] retrying conv={id}");
                let inner = store.inner();
                let mut inner = inner.lock().unwrap();
                if let Some(state) = inner.get_mut(&id) {
                    state.status = AgentStatus::Streaming;
                    bump(state);
                }
            }
            PipelineOutcome::Stop => {
                eprintln!("[agent] turn stopped conv={id}");
                store.task_handles().lock().unwrap().remove(&id);
                return;
            }
        }
    }
}

fn execute_text_turn_with_retries(
    provider: super::providers::AgentProviderKind,
    model: &str,
    system_prompt: &str,
    messages: &[AgentMessage],
    id: ConvId,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let client = AgentNetworkClient::load_default()?;
    let mut last_error = None::<String>;

    for attempt in 1..=MAX_REQUEST_ATTEMPTS {
        if cancel.load(Ordering::Acquire) {
            return Err("request cancelled".to_string());
        }

        let result = client
            .execute_text_turn(provider, model, system_prompt, messages)
            .map_err(|error| error.message);
        match result {
            Ok(text) => {
                if attempt > 1 {
                    eprintln!("[agent] request retry succeeded conv={id} attempt={attempt}");
                }
                return Ok(text);
            }
            Err(error) => {
                let retryable = is_retryable_request_error(&error);
                eprintln!(
                    "[agent] request attempt failed conv={id} attempt={attempt}/{MAX_REQUEST_ATTEMPTS} retryable={retryable}: {error}"
                );
                last_error = Some(error.clone());
                if !retryable || attempt == MAX_REQUEST_ATTEMPTS {
                    break;
                }
                std::thread::sleep(request_retry_delay(attempt));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "request failed".to_string()))
}

fn request_retry_delay(attempt: u8) -> Duration {
    Duration::from_millis(750 * attempt as u64)
}

fn is_retryable_request_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    if lower.contains("http 400")
        || lower.contains("http 401")
        || lower.contains("http 403")
        || lower.contains("missing required")
        || lower.contains("invalid_request_error")
        || lower.contains("invalid api key")
    {
        return false;
    }

    lower.contains("error sending request")
        || lower.contains("timeout=true")
        || lower.contains("connect=true")
        || lower.contains("operation timed out")
        || lower.contains("connection")
        || lower.contains("dns")
        || lower.contains("http 408")
        || lower.contains("http 409")
        || lower.contains("http 429")
        || lower.contains("http 500")
        || lower.contains("http 502")
        || lower.contains("http 503")
        || lower.contains("http 504")
}

struct TurnRequest {
    kind: AgentKind,
    provider: super::providers::AgentProviderKind,
    model: String,
    messages: Vec<AgentMessage>,
}

fn build_request(store: &ConversationStore, id: ConvId) -> Result<TurnRequest, String> {
    let inner = store.inner();
    let inner = inner.lock().unwrap();
    let state = inner
        .get(&id)
        .ok_or_else(|| format!("unknown agent conversation {id}"))?;
    Ok(TurnRequest {
        kind: state.kind,
        provider: state.provider,
        model: state.model.clone(),
        messages: state
            .messages
            .iter()
            .map(|message| AgentMessage {
                role: match message.role {
                    Role::User => AgentMessageRole::User,
                    Role::Assistant => AgentMessageRole::Assistant,
                    Role::System => AgentMessageRole::System,
                },
                content: message.text.clone(),
                tool_name: None,
            })
            .collect(),
    })
}

enum PipelineOutcome {
    Done,
    Retry,
    Stop,
}

fn run_post_turn_pipeline(
    store: &ConversationStore,
    id: ConvId,
    response: &str,
) -> PipelineOutcome {
    let artifacts = match instrument_artifacts(response) {
        Ok(artifacts) => artifacts,
        Err(error) => {
            eprintln!("[agent] missing required artifacts conv={id}: {error}");
            return retry_or_fail(store, id, error, |state, text| {
                state.last_compile_error = Some(text)
            });
        }
    };
    eprintln!(
        "[agent] extracted instrument artifacts conv={id} dsp_len={} ui_len={}\n[agent-source conv={id} dsp.lisp BEGIN]\n{}\n[agent-source conv={id} dsp.lisp END]\n[agent-source conv={id} ui.lisp BEGIN]\n{}\n[agent-source conv={id} ui.lisp END]",
        artifacts.dsp_source.len(),
        artifacts.ui_source.len(),
        artifacts.dsp_source,
        artifacts.ui_source
    );

    let kind = {
        let inner = store.inner();
        let mut inner = inner.lock().unwrap();
        let Some(state) = inner.get_mut(&id) else {
            return PipelineOutcome::Stop;
        };
        state.draft = None;
        state.last_compile_error = None;
        eprintln!(
            "[agent] conv={id} transition {:?} -> Compiling",
            state.status
        );
        state.status = AgentStatus::Compiling;
        bump(state);
        state.kind
    };

    match kind {
        AgentKind::Effect => {
            eprintln!("[agent] effect mode requested conv={id}; unsupported in V1");
            set_error(
                store,
                id,
                "effect agent mode is not implemented in V1".to_string(),
            );
            PipelineOutcome::Stop
        }
        AgentKind::Instrument => run_instrument_pipeline(store, id, artifacts),
    }
}

fn run_instrument_pipeline(
    store: &ConversationStore,
    id: ConvId,
    artifacts: InstrumentArtifacts,
) -> PipelineOutcome {
    let compile_result = match crate::lisp_effect::compile_and_load_instrument(
        &artifacts.dsp_source,
        store.sample_rate(),
    ) {
        Ok(result) => {
            eprintln!(
                "[agent] compile ok conv={id} params={} inputs={} outputs={}",
                result.manifest.params.len(),
                result.manifest.n_inputs,
                result.manifest.n_outputs
            );
            result
        }
        Err(error) => {
            eprintln!(
                    "[agent] compile failed conv={id}: {error}\n[agent-source failed conv={id} BEGIN]\n{}\n[agent-source failed conv={id} END]",
                    artifacts.dsp_source
                );
            return retry_or_fail(
                store,
                id,
                format!("compile error:\n{error}"),
                |state, text| state.last_compile_error = Some(text),
            );
        }
    };

    if let Err(error) =
        validate_instrument_ui_source(&artifacts.ui_source, &compile_result.manifest)
    {
        eprintln!(
            "[agent] ui validation failed conv={id}: {error}\n[agent-source failed conv={id} ui.lisp BEGIN]\n{}\n[agent-source failed conv={id} ui.lisp END]",
            artifacts.ui_source
        );
        return retry_or_fail(
            store,
            id,
            format!("ui.lisp validation error:\n{error}"),
            |state, text| state.last_compile_error = Some(text),
        );
    }
    eprintln!("[agent] ui validation ok conv={id}");

    {
        let inner = store.inner();
        let mut inner = inner.lock().unwrap();
        let Some(state) = inner.get_mut(&id) else {
            return PipelineOutcome::Stop;
        };
        eprintln!(
            "[agent] conv={id} transition {:?} -> Auditioning",
            state.status
        );
        state.status = AgentStatus::Auditioning;
        bump(state);
    }

    let audition = match audition_loaded_instrument(&compile_result, store.sample_rate()) {
        Ok(audition) => {
            eprintln!(
                "[agent] audition ok conv={id} peak_db={:.2} rms_db={:.2} clipped={} silent={}",
                audition.peak_db, audition.rms_db, audition.clipped, audition.silent
            );
            audition
        }
        Err(error) => {
            eprintln!("[agent] audition failed conv={id}: {error}");
            return retry_or_fail(store, id, format!("audition failed:\n{error}"), |_, _| {});
        }
    };
    let feedback = audition_feedback(&audition);
    {
        let inner = store.inner();
        let mut inner = inner.lock().unwrap();
        let Some(state) = inner.get_mut(&id) else {
            return PipelineOutcome::Stop;
        };
        state.draft = Some(InstrumentDraft {
            dsp_source: artifacts.dsp_source,
            ui_source: artifacts.ui_source,
        });
        state.last_audition = Some(audition.clone());
        push_message(state, Role::System, feedback.clone());
    }

    if audition.silent || audition.clipped {
        retry_or_fail(store, id, feedback, |_, _| {})
    } else {
        let inner = store.inner();
        let mut inner = inner.lock().unwrap();
        if let Some(state) = inner.get_mut(&id) {
            eprintln!("[agent] conv={id} transition {:?} -> Idle", state.status);
            state.retries_this_turn = 0;
            state.status = AgentStatus::Idle;
            bump(state);
        }
        PipelineOutcome::Done
    }
}

fn retry_or_fail<F>(
    store: &ConversationStore,
    id: ConvId,
    message: String,
    mut record_error: F,
) -> PipelineOutcome
where
    F: FnMut(&mut ConversationState, String),
{
    let inner = store.inner();
    let mut inner = inner.lock().unwrap();
    let Some(state) = inner.get_mut(&id) else {
        return PipelineOutcome::Stop;
    };
    eprintln!(
        "[agent] retry_or_fail conv={id} retry={}/{} message={:?}",
        state.retries_this_turn, MAX_RETRIES_PER_TURN, message
    );
    record_error(state, message.clone());
    push_message(state, Role::System, message);
    if state.retries_this_turn < MAX_RETRIES_PER_TURN {
        state.retries_this_turn += 1;
        bump(state);
        PipelineOutcome::Retry
    } else {
        eprintln!("[agent] retry budget exhausted conv={id}; status=Error");
        state.status = AgentStatus::Error;
        bump(state);
        PipelineOutcome::Stop
    }
}

fn set_error(store: &ConversationStore, id: ConvId, message: String) {
    eprintln!("[agent] set_error conv={id}: {message}");
    let inner = store.inner();
    let mut inner = inner.lock().unwrap();
    if let Some(state) = inner.get_mut(&id) {
        state.status = AgentStatus::Error;
        push_message(state, Role::System, message);
    }
    store.task_handles().lock().unwrap().remove(&id);
}

fn set_idle(store: &ConversationStore, id: ConvId) {
    eprintln!("[agent] set_idle conv={id}");
    let inner = store.inner();
    let mut inner = inner.lock().unwrap();
    if let Some(state) = inner.get_mut(&id) {
        state.status = AgentStatus::Idle;
        bump(state);
    }
}

fn system_prompt_for(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Instrument => include_str!("prompts/instrument.md"),
        AgentKind::Effect => include_str!("prompts/effect.md"),
    }
}

#[cfg(test)]
mod tests {
    use super::is_retryable_request_error;

    #[test]
    fn request_retry_classifier_skips_bad_request_schema_errors() {
        assert!(!is_retryable_request_error(
            "OpenAI Responses request failed: HTTP 400 Bad Request body: invalid_request_error",
        ));
    }

    #[test]
    fn request_retry_classifier_retries_transport_and_server_errors() {
        assert!(is_retryable_request_error(
            "OpenAI Responses request failed: error sending request for url",
        ));
        assert!(is_retryable_request_error(
            "OpenAI Responses request failed: HTTP 503 Service Unavailable",
        ));
    }
}
