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
pub mod workspace;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::Value;

pub use mcp_client::{McpClient, ServerHandle, ToolBackend, ToolResult};
pub use transcript::{clip, Entry, ToolStatus, Transcript};
use transcript::with_notes;
use turn::{push_user_text, record, run_turn, AnthropicTransport, TurnConfig};
pub use workspace::{Artifact, Change, FileChange, KindDecl};
use workspace::{Workspace, WRITE_TOOLS};

use crate::agent::providers::AgentProviderKind;

const DEFAULT_MODEL: &str = "claude-opus-5-5";

/// The models the pane offers: id and the name it shows.
pub const MODELS: &[(&str, &str)] = &[
    ("claude-fable-5-1", "Fable 5.1"),
    ("claude-opus-5-5", "Opus 5.5"),
    ("claude-sonnet-5-5", "Sonnet 5.5"),
    ("claude-haiku-5-5", "Haiku 5.5"),
];

/// Effort levels, least to most.
pub const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
/// Authoring is agentic work: Claude Opus 5.5 defaults to `medium`.
const DEFAULT_EFFORT: &str = "high";
const MAX_TOKENS: u32 = 64_000;
const MAX_TOOL_ROUNDS: usize = 100;

const PREAMBLE: &str = "You are the agent inside eseq, a music sequencer. The user talks to you \
in a pane beside their project and sees your tool calls as they run.\n\
\n\
Your tools run against a separate, headless eseq, not the user's open project. What you write \
with create_module, define_form or write_module goes to the Local packages folder, which the \
user's app reloads by itself.\n\
\n\
Put new work in one new Local package: a short lowercase name for what it is (`euclid`), with \
modules under it (`euclid.core`, `euclid.view`). To improve an existing package, edit its \
modules in place. The pane shows each package you write as a card with its files and changed \
lines and buttons to load it into the project, add its kinds, open its files, rename it or \
reject it, so don't list files or paths: say what you built and how to try it, briefly. A \
message may start with notes on what the user did in the pane since your last turn (renamed \
or rejected a package); take them as the current state.\n\
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
    /// One folder per conversation (named by its start, in Unix seconds):
    /// `conversation.json`, the messages so far, and the tools' images.
    pub conversations_dir: PathBuf,
    /// The Local packages folder, whose packages the pane tracks.
    pub local_root: PathBuf,
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
            conversations_dir: state_dir.join("conversations"),
            local_root: crate::app_paths::app_paths().local_modules_dir(),
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
    workspace: Mutex<Workspace>,
    /// The model and effort the next turn uses: (model id, effort).
    choice: Mutex<(String, String)>,
    /// What the user did in the pane since the last turn, for the next
    /// message to tell the agent.
    notes: Mutex<Vec<String>>,
    /// This conversation's folder, named when its first turn starts.
    conversation: Mutex<Option<PathBuf>>,
}

impl Shared {
    fn publish_artifacts(&self) {
        let artifacts = lock(&self.workspace).artifacts();
        lock(&self.transcript).set_artifacts(artifacts);
    }
}

/// One conversation with the agent. Cheap to clone; turns run on their own
/// thread.
#[derive(Clone)]
pub struct AgentSession {
    shared: Arc<Shared>,
}

impl AgentSession {
    pub fn new(config: SessionConfig) -> Self {
        let workspace = Workspace::new(config.local_root.clone());
        let choice = (config.model.clone(), config.effort.clone());
        Self {
            shared: Arc::new(Shared {
                config,
                transcript: Mutex::new(Transcript::default()),
                history: Mutex::new(Vec::new()),
                running: Mutex::new(None),
                server: Mutex::new(None),
                cancel: AtomicBool::new(false),
                workspace: Mutex::new(workspace),
                choice: Mutex::new(choice),
                notes: Mutex::new(Vec::new()),
                conversation: Mutex::new(None),
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
        self.reset_to(Transcript::default())
    }

    /// Start over, showing `next`. Under the transcript lock throughout, so
    /// no turn can start in between.
    fn reset_to(&self, next: Transcript) -> Result<(), String> {
        let mut transcript = lock(&self.shared.transcript);
        if transcript.busy {
            return Err("Stop the running turn first.".to_string());
        }
        lock(&self.shared.history).clear();
        lock(&self.shared.workspace).clear();
        lock(&self.shared.notes).clear();
        *lock(&self.shared.conversation) = None;
        let (generation, shape) = (transcript.generation + 1, transcript.shape + 1);
        *transcript = Transcript { generation, shape, ..next };
        Ok(())
    }

    /// Show a saved conversation (a `conversation.json`) in the pane, to
    /// read it back. Only shown: the next message starts a new conversation,
    /// since the saved copy has no image data to send.
    pub fn show_saved(&self, path: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let saved: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| format!("{} is not JSON: {error}", path.display()))?;
        let messages = saved["messages"].as_array().ok_or_else(|| format!("{} has no messages", path.display()))?;
        self.reset_to(Transcript::from_messages(messages))
    }

    /// This conversation's folder, once it has had a turn.
    pub fn conversation_dir(&self) -> Option<PathBuf> {
        lock(&self.shared.conversation).clone()
    }

    /// The model id and effort the next turn uses.
    pub fn choice(&self) -> (String, String) {
        lock(&self.shared.choice).clone()
    }

    /// Use `model` from the next turn on (one of [`MODELS`], or the
    /// override the session started with).
    pub fn set_model(&self, model: &str) -> Result<(), String> {
        if !MODELS.iter().any(|(id, _)| *id == model) && model != self.shared.config.model {
            return Err(format!("Unknown model {model}."));
        }
        lock(&self.shared.choice).0 = model.to_string();
        self.touch();
        Ok(())
    }

    /// Use `effort` (one of [`EFFORTS`]) from the next turn on.
    pub fn set_effort(&self, effort: &str) -> Result<(), String> {
        if !EFFORTS.contains(&effort) {
            return Err(format!("Unknown effort {effort}."));
        }
        lock(&self.shared.choice).1 = effort.to_string();
        self.touch();
        Ok(())
    }

    /// Redraw the pane: what it shows of the project changed.
    pub fn touch(&self) {
        lock(&self.shared.transcript).touch();
    }

    /// Put a package back as it was before this conversation wrote it (one
    /// the agent made is deleted).
    pub fn reject(&self, package: &str) -> Result<String, String> {
        self.edit_workspace(|workspace| workspace.reject(package))
    }

    /// Rename a package the agent made, folder and module names.
    pub fn rename(&self, package: &str, new_name: &str) -> Result<String, String> {
        if package == new_name.trim() {
            return Ok(format!("`{package}` keeps its name."));
        }
        self.edit_workspace(|workspace| workspace.rename(package, new_name))
    }

    fn edit_workspace(&self, edit: impl FnOnce(&mut Workspace) -> Result<String, String>) -> Result<String, String> {
        if self.busy() {
            return Err("Stop the running turn first.".to_string());
        }
        let message = edit(&mut lock(&self.shared.workspace))?;
        // The tool server has the old modules loaded: the next turn starts
        // a fresh one that reads the files as they are now.
        *lock(&self.shared.server) = None;
        *lock(&self.shared.running) = None;
        lock(&self.shared.notes).push(message.clone());
        lock(&self.shared.transcript).push(Entry::Notice(message.clone()));
        self.shared.publish_artifacts();
        Ok(message)
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
        dir: PathBuf::new(),
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
    (turn.model, turn.effort) = lock(&shared.choice).clone();
    turn.dir = lock(&shared.conversation)
        .get_or_insert_with(|| {
            let started = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
            shared.config.conversations_dir.join(started.to_string())
        })
        .clone();
    let notes = std::mem::take(&mut *lock(&shared.notes));
    let text = with_notes(&notes, text);
    let mut history = lock(&shared.history);
    push_user_text(&mut history, &text);
    let mut tools = Tracking { tools: client, shared };
    let result = run_turn(transport, &mut tools, turn, &mut history, &shared.transcript, &shared.cancel);
    record(turn, &history);
    result
}

/// The tool server, noting each package before a tool writes it and
/// publishing what changed after.
struct Tracking<'a> {
    tools: &'a mut McpClient,
    shared: &'a Shared,
}

impl ToolBackend for Tracking<'_> {
    fn tools(&mut self) -> Result<Vec<Value>, String> {
        self.tools.tools()
    }

    fn call(&mut self, name: &str, input: &Value) -> ToolResult {
        let module = input["module"].as_str().filter(|_| WRITE_TOOLS.contains(&name));
        if let Some(module) = module {
            lock(&self.shared.workspace).before_write(name, module);
        }
        let result = self.tools.call(name, input);
        // A write that failed put its file back.
        if module.is_some() && !result.is_error {
            self.shared.publish_artifacts();
        }
        result
    }
}
