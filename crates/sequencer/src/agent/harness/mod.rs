//! The in-app agent (bead eseq-maxc.5): Claude, streaming, calling the
//! authoring tools of a `metal_seq mcp` child process. A session holds one
//! conversation; the UI reads its [`Transcript`] and polls the generation.
//!
//! The tools run against the child's own headless eseq, never the live
//! session: what the agent builds lands in the Local packages folder, which
//! the app's hot reload picks up.

pub mod mcp_client;
pub mod message;
pub mod sse;
pub mod transcript;
pub mod turn;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::Value;

pub use mcp_client::{McpClient, ServerHandle, ToolBackend, ToolResult};
pub use transcript::{clip, Entry, ToolStatus, Transcript};
use turn::{push_user_text, run_turn, AnthropicTransport, TurnConfig};

use crate::agent::providers::AgentProviderKind;

const DEFAULT_MODEL: &str = "claude-opus-5-5";
/// Authoring is agentic work: Claude Opus 5.5 defaults to `medium`.
const DEFAULT_EFFORT: &str = "high";
const MAX_TOKENS: u32 = 64_000;
const MAX_TOOL_ROUNDS: usize = 100;

const PREAMBLE: &str = "You are the agent inside eseq, a music sequencer. The user talks to you \
in a pane beside their project and sees your tool calls as they run.\n\
\n\
Your tools run against a separate, headless eseq, not the user's open project. What you write \
with create_module, define_form or write_module goes to the Local packages folder, which the \
user's app reloads by itself. When you finish something, tell the user how to use it in their \
project (for a sequencer kind: the Packages tab, then New <kind>), and keep explanations short.\n\
\n\
The pane renders your replies as simple markdown: paragraphs, **bold**, `code`, fenced code \
blocks, # headings and one level of - or 1. lists. Nested lists are not supported; keep lists \
flat.";

/// A lock that outlives a panicked turn: the pane must keep drawing.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The program that serves the tools over MCP.
pub struct ToolServer {
    pub program: PathBuf,
    pub args: Vec<String>,
}

pub struct SessionConfig {
    pub model: String,
    pub effort: String,
    pub tool_server: ToolServer,
    /// The tool server's stderr.
    pub log_path: PathBuf,
    /// Where tool images are saved for the pane.
    pub image_dir: PathBuf,
}

impl SessionConfig {
    /// The running executable as the tool server (`metal_seq mcp`), the
    /// model from Agent Mode's Anthropic override or Claude Opus 5.5.
    pub fn for_this_app(state_dir: PathBuf) -> Result<Self, String> {
        let program = std::env::current_exe().map_err(|error| error.to_string())?;
        let model = std::env::var(AgentProviderKind::Anthropic.model_override_env())
            .ok()
            .filter(|model| !model.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        Ok(Self {
            model,
            effort: DEFAULT_EFFORT.to_string(),
            tool_server: ToolServer { program, args: vec!["mcp".to_string()] },
            log_path: state_dir.join("agent-tools.log"),
            image_dir: state_dir.join("agent-images"),
        })
    }
}

/// What a turn needs, made on the first turn and kept: the API client (so
/// its connection stays warm), the tool server and the request settings.
struct Running {
    transport: AnthropicTransport,
    client: McpClient,
    turn: TurnConfig,
}

struct Shared {
    config: SessionConfig,
    transcript: Mutex<Transcript>,
    history: Mutex<Vec<Value>>,
    running: Mutex<Option<Running>>,
    /// The tool server, outside `running` (which a turn holds) so Stop can
    /// end a call that hangs.
    server: Mutex<Option<ServerHandle>>,
    cancel: AtomicBool,
}

/// One conversation with the agent. Cheap to clone; turns run on their own
/// thread.
#[derive(Clone)]
pub struct AgentSession {
    shared: Arc<Shared>,
}

impl AgentSession {
    pub fn new(config: SessionConfig) -> Self {
        // Images belong to a conversation; a new session starts without them.
        let _ = std::fs::remove_dir_all(&config.image_dir);
        Self {
            shared: Arc::new(Shared {
                config,
                transcript: Mutex::new(Transcript::default()),
                history: Mutex::new(Vec::new()),
                running: Mutex::new(None),
                server: Mutex::new(None),
                cancel: AtomicBool::new(false),
            }),
        }
    }

    /// Read the transcript without copying it.
    pub fn with_transcript<R>(&self, read: impl FnOnce(&Transcript) -> R) -> R {
        read(&lock(&self.shared.transcript))
    }

    pub fn generation(&self) -> u64 {
        self.with_transcript(|transcript| transcript.generation)
    }

    pub fn busy(&self) -> bool {
        self.with_transcript(|transcript| transcript.busy)
    }

    /// Stop the running turn at its next event or tool call; a tool call
    /// already waiting is cut off (its server restarts on the next turn).
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
        if let Some(server) = lock(&self.shared.server).as_ref() {
            server.interrupt();
        }
    }

    /// Start over: an empty conversation (the tool server keeps running).
    pub fn reset(&self) -> Result<(), String> {
        if self.busy() {
            return Err("Stop the running turn first.".to_string());
        }
        lock(&self.shared.history).clear();
        let _ = std::fs::remove_dir_all(&self.shared.config.image_dir);
        let mut transcript = lock(&self.shared.transcript);
        let generation = transcript.generation + 1;
        *transcript = Transcript { generation, ..Transcript::default() };
        Ok(())
    }

    /// Send the user's message and run the turn in the background.
    pub fn send(&self, text: &str) -> Result<(), String> {
        let text = text.trim().to_string();
        if text.is_empty() {
            return Err("Nothing to send.".to_string());
        }
        {
            let mut transcript = lock(&self.shared.transcript);
            if transcript.busy {
                return Err("The agent is still working; stop it or wait.".to_string());
            }
            transcript.push(Entry::User(text.clone()));
            transcript.set_busy(true);
        }
        self.shared.cancel.store(false, Ordering::Relaxed);
        let shared = Arc::clone(&self.shared);
        std::thread::Builder::new()
            .name("eseq-agent-turn".to_string())
            .spawn(move || {
                let _idle = IdleOnExit(&shared);
                if let Err(error) = take_turn(&shared, &text) {
                    lock(&shared.transcript).push(Entry::Notice(error));
                }
            })
            .map(|_| ())
            .map_err(|error| {
                lock(&self.shared.transcript).set_busy(false);
                error.to_string()
            })
    }
}

/// Clears `busy` however the turn thread ends, a panic included.
struct IdleOnExit<'a>(&'a Shared);

impl Drop for IdleOnExit<'_> {
    fn drop(&mut self) {
        lock(&self.0.transcript).set_busy(false);
    }
}

fn start(shared: &Shared) -> Result<Running, String> {
    let transport = AnthropicTransport::from_env()?;
    lock(&shared.transcript).push(Entry::Notice("Starting the authoring tools…".to_string()));
    let config = &shared.config;
    let mut client = McpClient::spawn(&config.tool_server.program, &config.tool_server.args, &config.log_path)?;
    let tools = client.tools()?;
    let turn = TurnConfig {
        model: config.model.clone(),
        effort: config.effort.clone(),
        max_tokens: MAX_TOKENS,
        max_rounds: MAX_TOOL_ROUNDS,
        system: format!("{PREAMBLE}\n\n{}", client.instructions),
        tools,
        image_dir: config.image_dir.clone(),
    };
    Ok(Running { transport, client, turn })
}

fn take_turn(shared: &Shared, text: &str) -> Result<(), String> {
    let mut running = lock(&shared.running);
    if running.as_ref().is_some_and(|running| !running.client.alive()) {
        *running = None;
    }
    if running.is_none() {
        let started = start(shared)?;
        *lock(&shared.server) = Some(started.client.handle());
        *running = Some(started);
    }
    let Running { transport, client, turn } = running.as_mut().expect("started above");
    let mut history = lock(&shared.history);
    push_user_text(&mut history, text);
    run_turn(transport, client, turn, &mut history, &shared.transcript, &shared.cancel)
}
