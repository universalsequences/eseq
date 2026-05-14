use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::actions::{AgentAppAction, AgentSessionContext};
use super::audition::{audition_feedback, audition_loaded_instrument};
use super::dsp_validate::validate_instrument_dsp_source;
use super::network::{AgentNetworkClient, AgentTurnResult};
use super::parse::{instrument_artifacts, last_dgenlisp_block, InstrumentArtifacts};
use super::protocol::ToolCall;
use super::providers::{AgentMessage, AgentMessageRole};
use super::store::{
    bump, push_message, push_message_with_reasoning, AgentKind, AgentStatus, ConvId,
    ConversationState, ConversationStore, InstrumentDraft, Role, RunningTask,
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
        let turn = match execute_tool_turn_with_retries(
            &store,
            request.provider,
            &request.model,
            system_prompt,
            &request.messages,
            id,
            &cancel,
        ) {
            Ok(turn) => {
                let has_dsp = last_dgenlisp_block(&turn.text).is_some();
                let has_artifacts = instrument_artifacts(&turn.text).is_ok();
                eprintln!(
                    "[agent] request ok conv={id} response_len={} has_dgenlisp_block={} has_required_artifacts={}",
                    turn.text.len(),
                    has_dsp,
                    has_artifacts
                );
                turn
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
            if !turn.text.trim().is_empty() {
                push_message_with_reasoning(
                    state,
                    Role::Assistant,
                    turn.text.clone(),
                    turn.reasoning_content.clone(),
                );
            }
            for outcome in &turn.tool_outcomes {
                push_message(
                    state,
                    Role::System,
                    format!(
                        "tool {} [{}]\n{}",
                        outcome.name,
                        if outcome.ok { "ok" } else { "error" },
                        outcome.summary
                    ),
                );
            }
        }

        match run_pending_action_pipeline(&store, id, turn.pending_actions) {
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

fn execute_tool_turn_with_retries(
    store: &ConversationStore,
    provider: super::providers::AgentProviderKind,
    model: &str,
    system_prompt: &str,
    messages: &[AgentMessage],
    id: ConvId,
    cancel: &AtomicBool,
) -> Result<AgentTurnResult, String> {
    let client = AgentNetworkClient::load_default()?;
    let mut last_error = None::<String>;

    for attempt in 1..=MAX_REQUEST_ATTEMPTS {
        if cancel.load(Ordering::Acquire) {
            return Err("request cancelled".to_string());
        }

        let result = client
            .execute_turn_with_progress(
                provider,
                model,
                system_prompt,
                messages,
                AgentSessionContext::default(),
                Some(&|call| {
                    let message = tool_progress_message(call);
                    eprintln!("[agent] tool progress conv={id}: {message}");
                    if let Err(error) = store.push_tool_message(id, message) {
                        eprintln!("[agent] failed to record tool progress conv={id}: {error}");
                    }
                }),
            )
            .map_err(|error| error.message);
        match result {
            Ok(turn) => {
                if attempt > 1 {
                    eprintln!("[agent] request retry succeeded conv={id} attempt={attempt}");
                }
                return Ok(turn);
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

fn tool_progress_message(call: &ToolCall) -> String {
    match call.name.as_str() {
        "lookup_dgen_docs" => {
            let queries = string_list_arg(&call.arguments, "queries")
                .or_else(|| string_arg(&call.arguments, "query").map(|query| vec![query]));
            match queries {
                Some(queries) if !queries.is_empty() => {
                    format!("Looking up DGenLisp docs for {}.", backtick_join(&queries))
                }
                _ => "Looking up DGenLisp docs.".to_string(),
            }
        }
        "list_examples" => match string_arg(&call.arguments, "kind") {
            Some(kind) => format!("Listing {kind} examples."),
            None => "Listing examples.".to_string(),
        },
        "read_example" => match string_arg(&call.arguments, "name") {
            Some(name) => format!("Reading example `{name}`."),
            None => "Reading an example.".to_string(),
        },
        "read_patch_source" => {
            let kind = string_arg(&call.arguments, "kind").unwrap_or_else(|| "patch".to_string());
            match string_arg(&call.arguments, "name") {
                Some(name) => format!("Reading {kind} source `{name}`."),
                None => format!("Reading {kind} source."),
            }
        }
        "list_instruments" => "Listing saved instruments.".to_string(),
        "read_instrument_source" | "read_current_instrument_source" => {
            match string_arg(&call.arguments, "name") {
                Some(name) => format!("Reading instrument `{name}`."),
                None => "Reading the current instrument source.".to_string(),
            }
        }
        "list_effects" => "Listing saved effects.".to_string(),
        "read_effect_source" | "read_current_effect_source" => {
            match string_arg(&call.arguments, "name") {
                Some(name) => format!("Reading effect `{name}`."),
                None => "Reading the current effect source.".to_string(),
            }
        }
        "create_instrument_artifact" => match string_arg(&call.arguments, "name") {
            Some(name) => format!("Creating draft instrument artifact `{name}`."),
            None => "Creating a draft instrument artifact.".to_string(),
        },
        "create_instrument_track" => match string_arg(&call.arguments, "name") {
            Some(name) => format!("Creating instrument track `{name}`."),
            None => "Creating an instrument track.".to_string(),
        },
        "update_current_instrument" => match string_arg(&call.arguments, "name") {
            Some(name) => format!("Updating current instrument as `{name}`."),
            None => "Updating the current instrument.".to_string(),
        },
        "inspect_current_instrument_preset_schema" => {
            "Inspecting the current instrument preset schema.".to_string()
        }
        "create_current_instrument_presets" => "Creating instrument presets.".to_string(),
        "apply_effect_to_current_track" => match string_arg(&call.arguments, "name") {
            Some(name) => format!("Applying effect `{name}` to the current track."),
            None => "Applying an effect to the current track.".to_string(),
        },
        "update_current_effect" => match string_arg(&call.arguments, "name") {
            Some(name) => format!("Updating current effect as `{name}`."),
            None => "Updating the current effect.".to_string(),
        },
        other => format!("Calling tool `{other}`."),
    }
}

fn string_arg(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn string_list_arg(value: &Value, key: &str) -> Option<Vec<String>> {
    let items = value.get(key)?.as_array()?;
    let strings = items
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    (!strings.is_empty()).then_some(strings)
}

fn backtick_join(values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("`{value}`"))
        .collect::<Vec<_>>()
        .join(", ")
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
            .filter_map(|message| {
                let role = match message.role {
                    Role::User => AgentMessageRole::User,
                    Role::Assistant => AgentMessageRole::Assistant,
                    Role::System => AgentMessageRole::System,
                    Role::Tool => return None,
                };
                Some(AgentMessage {
                    role,
                    content: message.text.clone(),
                    tool_name: None,
                    reasoning_content: message.reasoning_content.clone(),
                })
            })
            .collect(),
    })
}

enum PipelineOutcome {
    Done,
    Retry,
    Stop,
}

fn run_pending_action_pipeline(
    store: &ConversationStore,
    id: ConvId,
    actions: Vec<AgentAppAction>,
) -> PipelineOutcome {
    if actions.is_empty() {
        let inner = store.inner();
        let mut inner = inner.lock().unwrap();
        if let Some(state) = inner.get_mut(&id) {
            state.retries_this_turn = 0;
            state.status = AgentStatus::Idle;
            bump(state);
        }
        return PipelineOutcome::Done;
    }

    for action in actions {
        match apply_agent_action(store, id, action) {
            Ok(message) => {
                let inner = store.inner();
                let mut inner = inner.lock().unwrap();
                if let Some(state) = inner.get_mut(&id) {
                    push_message(state, Role::System, message);
                }
            }
            Err(error) => {
                return retry_or_fail(store, id, error, |state, text| {
                    state.last_compile_error = Some(text)
                });
            }
        }
    }

    let inner = store.inner();
    let mut inner = inner.lock().unwrap();
    if let Some(state) = inner.get_mut(&id) {
        state.retries_this_turn = 0;
        state.status = AgentStatus::Idle;
        bump(state);
    }
    PipelineOutcome::Done
}

fn apply_agent_action(
    store: &ConversationStore,
    id: ConvId,
    action: AgentAppAction,
) -> Result<String, String> {
    match action {
        AgentAppAction::CreateInstrumentArtifact {
            name,
            dsp_source,
            ui_source,
        } => create_instrument_artifact(store, id, name, dsp_source, ui_source),
        other => Err(format!(
            "Tool returned unsupported action for this agent panel: {other:?}"
        )),
    }
}

fn create_instrument_artifact(
    store: &ConversationStore,
    id: ConvId,
    name: String,
    dsp_source: String,
    ui_source: String,
) -> Result<String, String> {
    if let Err(error) = validate_instrument_dsp_source(&dsp_source) {
        log_failed_instrument_sources(id, "dsp validation", &error, &dsp_source, &ui_source);
        return Err(format!("dsp.lisp validation error:\n{error}"));
    }

    let compile_result =
        match crate::lisp_effect::compile_and_load_instrument(&dsp_source, store.sample_rate()) {
            Ok(result) => result,
            Err(error) => {
                log_failed_instrument_sources(id, "dsp compile", &error, &dsp_source, &ui_source);
                return Err(format!("compile error:\n{error}"));
            }
        };

    if let Err(error) = validate_instrument_ui_source(&ui_source, &compile_result.manifest) {
        log_failed_instrument_sources(id, "ui validation", &error, &dsp_source, &ui_source);
        return Err(format!("ui.lisp validation error:\n{error}"));
    }

    let audition = match audition_loaded_instrument(&compile_result, store.sample_rate()) {
        Ok(audition) => audition,
        Err(error) => {
            log_failed_instrument_sources(id, "audition", &error, &dsp_source, &ui_source);
            return Err(format!("audition failed:\n{error}"));
        }
    };
    let feedback = audition_feedback(&audition);
    if audition.silent || audition.clipped {
        return Err(feedback);
    }

    let inner = store.inner();
    let mut inner = inner.lock().unwrap();
    let state = inner
        .get_mut(&id)
        .ok_or_else(|| format!("unknown agent conversation {id}"))?;
    state.draft = Some(InstrumentDraft {
        dsp_source,
        ui_source,
    });
    state.last_audition = Some(audition);
    state.last_compile_error = None;
    state.finalized_instrument_name = None;
    state.status = AgentStatus::Idle;
    bump(state);
    Ok(format!(
        "Created validated draft instrument artifact '{}'. {feedback}",
        name
    ))
}

fn log_failed_instrument_sources(
    id: ConvId,
    stage: &str,
    error: &str,
    dsp_source: &str,
    ui_source: &str,
) {
    eprintln!(
        "[agent] instrument artifact failed conv={id} stage={stage}: {error}\n[agent-source failed conv={id} dsp.lisp BEGIN]\n{dsp_source}\n[agent-source failed conv={id} dsp.lisp END]\n[agent-source failed conv={id} ui.lisp BEGIN]\n{ui_source}\n[agent-source failed conv={id} ui.lisp END]"
    );
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
    if let Err(error) = validate_instrument_dsp_source(&artifacts.dsp_source) {
        eprintln!(
            "[agent] dsp validation failed conv={id}: {error}\n[agent-source failed conv={id} BEGIN]\n{}\n[agent-source failed conv={id} END]",
            artifacts.dsp_source
        );
        return retry_or_fail(
            store,
            id,
            format!("dsp.lisp validation error:\n{error}"),
            |state, text| state.last_compile_error = Some(text),
        );
    }

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
    let retry_message = message_with_retry_guidance(&message);
    record_error(state, retry_message.clone());
    push_message(state, Role::System, retry_message);
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

fn message_with_retry_guidance(message: &str) -> String {
    if !is_artifact_repair_error(message) {
        return message.to_string();
    }

    format!(
        "{message}\n\nRetry instruction: repair the exact full artifact you just generated and call `create_instrument_artifact` again. Do not call `list_examples`, `read_example`, or `read_instrument_source` again for this direct validator/compiler error unless the error is about unknown syntax/operator and does not already provide the replacement. Do not reread an example you already read in this conversation."
    )
}

fn is_artifact_repair_error(message: &str) -> bool {
    message.starts_with("dsp.lisp validation error:")
        || message.starts_with("compile error:")
        || message.starts_with("ui.lisp validation error:")
        || message.starts_with("audition failed:")
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
    use super::{
        build_request, is_retryable_request_error, message_with_retry_guidance,
        tool_progress_message,
    };
    use crate::agent::protocol::ToolCall;
    use crate::agent::store::{ConversationStore, Role};
    use serde_json::json;

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

    #[test]
    fn tool_progress_message_names_read_example() {
        let message = tool_progress_message(&ToolCall {
            name: "read_example".to_string(),
            arguments: json!({ "name": "emulations/prophet-5" }),
        });
        assert_eq!(message, "Reading example `emulations/prophet-5`.");
    }

    #[test]
    fn build_request_filters_tool_progress_messages() {
        let store = ConversationStore::new(48_000);
        let id = store.new_conversation(crate::agent::store::AgentKind::Instrument);
        {
            let inner = store.inner();
            let mut inner = inner.lock().unwrap();
            let state = inner.get_mut(&id).unwrap();
            crate::agent::store::push_message(state, Role::User, "make a bass".to_string());
            crate::agent::store::push_message(
                state,
                Role::Tool,
                "Reading example `minimoog`.".to_string(),
            );
            crate::agent::store::push_message(state, Role::System, "compile error".to_string());
        }

        let request = build_request(&store, id).unwrap();
        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[0].content, "make a bass");
        assert_eq!(request.messages[1].content, "compile error");
    }

    #[test]
    fn artifact_errors_include_direct_repair_guidance() {
        let message = message_with_retry_guidance(
            "ui.lisp validation error:\nUnknownVariable(\"ui-accent-magenta\")",
        );
        assert!(message.contains("repair the exact full artifact"));
        assert!(message.contains(
            "Do not call `list_examples`, `read_example`, or `read_instrument_source` again"
        ));
    }

    #[test]
    fn non_artifact_errors_do_not_include_repair_guidance() {
        let message = message_with_retry_guidance("request failed: HTTP 503");
        assert_eq!(message, "request failed: HTTP 503");
    }
}
