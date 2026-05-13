# Agent Mode Spec

## Purpose

Bring back the long-dormant "agent mode" from the TUI era and integrate it into
the eseqlisp UI to let an LLM author **custom instruments** and **custom
effects** in dgenlisp. Manual dgenlisp authoring already works — this spec is
about removing the friction of writing it by hand for users who would rather
describe what they want.

The agent runs entirely in Rust against the project's existing generic
Responses API surface (no vendor SDK). eseqlisp drives the UI synchronously
and observes the agent's state reactively.

## Design Goals

- Keep eseqlisp synchronous. All async work lives in Rust.
- Provider-agnostic. Reuse the existing Responses API client; no
  Anthropic-specific code paths.
- The generated code is the primary artifact and streams into the UI as
  visible text, not as a tool-call payload.
- The "work in progress" instrument/effect is a real, mounted, audible
  instance — not a parallel preview type — flagged as a draft until the user
  Saves.
- The agent self-corrects compile and silent-output errors automatically
  within a bounded retry budget; the user is only interrupted when the agent
  cannot make progress.
- Conversation state is durable across UI ticks and observable via cheap
  reactive readers.

## Non-Goals (V1)

- Multi-conversation parallelism in the UI (the store supports it, the UI
  shows one at a time).
- Editing existing user-written instruments via the agent (V1 = greenfield
  authoring only; iterating on the agent's own draft is in scope).
- AST-level edit tools. V1 is full-rewrite oneshot per turn.
- Patch-cable AST visualization of the in-progress code.
- FFT / musicality analysis in audition.
- Effect input-signal probing beyond a basic impulse/noise burst.

## Architecture

### Layering

```
┌─────────────────────────────────────────┐
│ eseqlisp UI (synchronous)               │
│   - center-pane AgentBuffer mode        │
│   - bottom-pane draft instrument render │
│   - reads reactive state each tick      │
└──────────────┬──────────────────────────┘
               │ lisp builtins (commands + readers)
┌──────────────▼──────────────────────────┐
│ Rust agent module (src/agent/)          │
│   ConversationStore (Arc<Mutex<…>>)     │
│   per-conversation tokio task           │
│   post-turn pipeline (compile/audition) │
│   Responses API client (existing)       │
└─────────────────────────────────────────┘
```

### Single-writer rule

Each conversation's state is mutated only by its owning task. Lisp readers
take cheap snapshots under a short-held lock. UI never blocks on the agent.

### Background runtime

A single multi-threaded tokio runtime owns all conversation tasks. If a tokio
runtime already exists in the process, reuse it; otherwise spin one up at
agent module init.

## Conversation State

```rust
pub enum AgentKind { Instrument, Effect }

pub enum AgentStatus {
    Idle,
    Streaming,
    Compiling,
    Auditioning,
    Error,
    Cancelled,
}

pub struct Message {
    pub role: Role,          // User | Assistant | System
    pub text: String,
    pub ts: SystemTime,
}

pub struct ConversationState {
    pub id: ConvId,
    pub kind: AgentKind,
    pub status: AgentStatus,
    pub messages: Vec<Message>,
    pub draft_source: Option<String>,    // current dgenlisp source
    pub draft_handle: Option<DraftSlot>, // mounted instance, if compile ok
    pub last_compile_error: Option<String>,
    pub last_audition: Option<AuditionResult>,
    pub retries_this_turn: u8,
    pub generation: u64,                 // bumped on every mutation
    pub created_at: SystemTime,
}

pub struct AuditionResult {
    pub ran: bool,
    pub peak_db: f32,
    pub rms_db: f32,
    pub clipped: bool,
    pub duration_ms: u32,
}
```

`generation` is the cheap dirty-check the UI uses — store last-seen gen,
re-render only when it bumps.

### Conversation store

```rust
pub struct ConversationStore {
    inner: Arc<Mutex<HashMap<ConvId, ConversationState>>>,
    runtime: tokio::runtime::Handle,
    task_handles: Mutex<HashMap<ConvId, JoinHandle<()>>>,
}
```

One global `ConversationStore` lives in `App` (or wherever shared mutable
state lives today).

## Lisp Surface

### Commands (return immediately, fire-and-forget)

| Builtin | Returns | Effect |
|---|---|---|
| `(agent/new :kind 'instrument)` | conv-id | creates conversation, returns id |
| `(agent/new :kind 'effect)` | conv-id | same, effect mode |
| `(agent/send conv-id "prompt")` | nil | enqueues user message, starts task if idle |
| `(agent/cancel conv-id)` | nil | drops in-flight task, sets `Cancelled` |
| `(agent/accept conv-id)` | track-or-slot-id | promotes draft into a real slot |
| `(agent/discard conv-id)` | nil | drops draft, keeps conversation |
| `(agent/close conv-id)` | nil | tears down conversation entirely |

### Readers (cheap, snapshot)

| Builtin | Returns |
|---|---|
| `(agent/list)` | list of conv-ids |
| `(agent/kind conv-id)` | `'instrument` or `'effect` |
| `(agent/status conv-id)` | `'idle 'streaming 'compiling 'auditioning 'error 'cancelled` |
| `(agent/messages conv-id)` | list of `(:role :text :ts)` plists |
| `(agent/draft-source conv-id)` | string or nil |
| `(agent/draft-handle conv-id)` | mountable instance handle or nil |
| `(agent/last-error conv-id)` | string or nil |
| `(agent/last-audition conv-id)` | plist `(:ran :peak-db :rms-db :clipped :duration-ms)` or nil |
| `(agent/generation conv-id)` | u64 |

All readers are O(1) under a short-held lock. None block on agent work.

## Post-Turn Pipeline

Every assistant turn ends when the model stops streaming. The pipeline runs
unconditionally — the model never decides whether to compile or audition.

```
assistant turn ends
  ↓
parse last fenced ```dgenlisp block from message text
  ├─ no block found → status = Idle, return to user
  └─ block found → store as draft_source, status = Compiling
       ↓
       compile_draft(source)
       ├─ err → record last_compile_error
       │        if retries_this_turn < 3:
       │          inject system message with error,
       │          retries_this_turn += 1, status = Streaming, loop
       │        else:
       │          status = Error, surface to user
       └─ ok  → mount draft into draft slot, status = Auditioning
            ↓
            audition()  (auto-invoked; not a model tool)
            ├─ silent (peak < -60dB) or clipped:
            │    inject system message with metrics,
            │    if retries_this_turn < 3: retry, else surface
            └─ ran clean:
                 inject system message with metrics,
                 reset retries_this_turn = 0,
                 status = Idle
```

System messages are appended to `messages` so the user can see the agent's
self-correction history. They are also part of the next request's context.

### Retry budget

`retries_this_turn` is reset to 0 whenever:
- A new user message arrives.
- An audition succeeds.

It is incremented on every compile or silent-audition failure that triggers a
retry. Hard cap = 3. After that, status flips to `Error` and the user must
respond.

## Tool Surface (Model-Visible)

Code is **not** a tool. It streams as fenced ` ```dgenlisp ` text and is
extracted by the post-turn parser.

The model sees only side-effect tools:

| Tool | Purpose | Notes |
|---|---|---|
| `search_examples(query: string)` | Retrieve dgenlisp docs / example instruments / example effects | RAG over indexed corpus; only wire if a corpus exists, otherwise omit |

`compile_draft` and `audition` are **not** model-visible tools. They run
automatically in the post-turn pipeline. Removing them from the model's tool
list prevents the agent from fiddling with build/test as if it were a
decision.

If observation reveals the model would benefit from explicit invocation
(e.g., "audition with these specific test notes"), promote them to tools
later.

## Audition

Implemented as a shell-out to the existing `instrument-probe` executable.

### For instruments

- Probe plays a short note sequence (existing behavior).
- Parse stdout for peak/RMS/clipped/duration.
- Feedback string injected into messages:
  - clean: `audition: peak -3.2 dB, rms -14.1 dB, ran 600ms`
  - silent: `audition: SILENT (peak -inf). Likely no signal — check envelope, gain stage, or signal routing.`
  - clipped: `audition: CLIPPED (peak +2.1 dB). Reduce gain.`

### For effects

- Probe feeds the effect a short pink-noise or impulse burst.
- Reports peak/RMS of output AND a "differs from input" flag (simple
  per-sample diff energy). Passthrough or no-op effects fail this check.
- This is the one extension to `instrument-probe` that V1 needs.

### Auto-invocation

Audition runs after every successful compile inside the post-turn pipeline.
The model never invokes it directly.

## Draft Slots

A draft slot is a normal `InstrumentType::Custom` slot (or effect equivalent)
with two flags:

```rust
pub struct DraftSlot {
    pub conv_id: ConvId,
    pub kind: AgentKind,
    pub mounted: MountedInstance, // same type as a real slot
    pub draft: bool,              // excluded from PatternSnapshot
}
```

- Draft slots route through the audio graph identically to real slots so
  audition produces real audio.
- Draft slots are excluded from `PatternSnapshot` save/restore.
- The bottom pane renders draft instruments using the same code path as
  real custom instruments — no preview-specific UI needed. Custom
  instruments already self-describe their parameters.
- A small "Save" button rendered next to the draft instrument calls
  `(agent/accept conv-id)`.

### Accept

`(agent/accept conv-id)`:
1. Atomically swaps the draft's mounted instance into a chosen real track
   slot (instrument) or effect chain position (effect).
2. Clears the `draft` flag.
3. Removes the draft slot from the conversation.
4. Returns the destination id.

User selects the destination slot in the UI before accepting (e.g., "save to
track 3" or "save to bus A fx slot 2").

### Discard

`(agent/discard conv-id)` unmounts the draft, drops the slot, keeps the
conversation alive (so the user can iterate again).

## UI

### Center-pane mode: AgentBuffer

Replaces the Cirklon grid when active. Built from native eseqlisp widgets
(boxes, labels, stacks, dropdown, button, scroll-view), **not** a
monospace text buffer. The aesthetic should match the rest of the app —
proportional type, real spacing, real components.

#### Header row

A single `h-stack` containing:
- Conversation kind tag (label, e.g. "Instrument" / "Effect") in an
  accent-colored pill.
- Conversation title (label, defaults to the first user message truncated;
  editable later).
- Model dropdown (existing dropdown widget, see Model Selection above).
- Status chip — small colored pill whose label and color come from
  `(agent/status conv-id)`:
  - `idle` — neutral grey, "ready"
  - `streaming` — blue, "thinking…"
  - `compiling` — amber, "compiling…"
  - `auditioning` — amber, "checking audio…"
  - `error` — red, "error"
  - `cancelled` — neutral, "cancelled"

#### Message list

Vertical `scroll-view` of message cards. Each message is a `box` widget,
**not** a text line. Cards differ by role:

- **User card**: right-aligned (or left-aligned with a subtle accent
  border), proportional text in a `label` widget. Compact padding.
- **Assistant card**: left-aligned, larger padding, may contain mixed
  content:
  - Prose paragraphs as plain `label` widgets (proportional font).
  - Code blocks as a dedicated `code-block` sub-widget (monospace ONLY
    inside this widget — the rest of the card is proportional). Renders
    with the existing dgenlisp syntax highlighter, line numbers optional,
    and a small "copy" affordance in the corner.
- **System card**: muted background, smaller font, distinct icon (e.g. a
  gear or compile/audio glyph). Used for compile errors, audition
  feedback, and pipeline status. Visually clearly separate from
  conversation prose so they don't read as the model talking.

Streaming assistant content updates the last assistant card in place as
text arrives. A subtle pulsing cursor or animated dot at the tail
indicates streaming.

#### Composer (bottom of pane)

`h-stack` with:
- Multi-line text input (existing input widget, auto-grows up to N lines).
- Send button (primary style). Disabled while status is `Streaming`,
  `Compiling`, or `Auditioning`.
- Cancel button (destructive style). Visible only while status is
  `Streaming`, `Compiling`, or `Auditioning`.

Enter sends; Shift+Enter inserts a newline.

#### Empty state

When a conversation has no messages yet, the message list area shows a
centered empty-state card: kind-appropriate prompt suggestions (e.g.
"Try: 'a glassy FM pad with slow attack'") rendered as clickable chips
that pre-fill the composer.

#### Widget reuse

All of the above should compose from the existing eseqlisp widget
vocabulary used elsewhere (sidebar, params pane, mixer). The agent buffer
introduces only one new sub-widget: `code-block` (highlighted dgenlisp
with copy affordance). If a similar highlighter is already used by
`ui.lisp` editors elsewhere, reuse it directly.

### Bottom pane: draft instrument/effect

While a draft exists:
- Bottom pane renders the draft using the existing custom-instrument UI
  pipeline (`ui.lisp` if defined, autogenerated otherwise).
- A `[Save to…]` button renders adjacent. Clicking opens a small slot
  picker, then calls `(agent/accept)`.
- A `[Discard]` button calls `(agent/discard)`.

### Entering AgentBuffer mode

Two entry points (V1):
- New keybinding (TBD; suggest `Ctrl+G`) opens an instrument picker
  ("instrument or effect?") then enters AgentBuffer with a fresh
  conversation.
- From the existing instrument picker (`Ctrl+N`), add an "Ask agent" option
  alongside "Sampler" / "Custom".

## Reactivity

eseqlisp UI re-renders on a tick. To avoid wasted work:

```lisp
(let ((gen (agent/generation conv-id)))
  (when (not (= gen *last-seen-gen*))
    (setf *last-seen-gen* gen)
    (rebuild-agent-buffer-view conv-id)))
```

The agent task bumps `generation` on every mutation: new message, status
change, new draft, new audition result.

## Model Selection

Per-conversation, selected via the existing dropdown widget rendered in the
AgentBuffer header next to the status indicator.

### State

```rust
pub struct ConversationState {
    // ...existing fields...
    pub model: ModelId,   // e.g. "claude-opus-4-7", "claude-sonnet-4-6"
}
```

`ModelId` is whatever string the Responses API client already accepts —
agent mode does not maintain its own model registry.

### Available models

The dropdown is populated from a single source of truth:

```rust
pub fn available_models() -> Vec<ModelId>
```

Lives next to the Responses API client (so adding a new model is one edit,
not two). If the client already exposes a model list, reuse it; otherwise
add this function alongside it.

### Default

A `default_model()` function returns the model used for new conversations.
V1: hard-coded constant (suggest `claude-sonnet-4-6` as a balance of cost
and quality for code authoring). V2 candidate: settings-file override.

### Lisp surface

| Builtin | Returns | Effect |
|---|---|---|
| `(agent/models)` | list of model-id strings | exposes `available_models()` |
| `(agent/model conv-id)` | current model-id string | reader |
| `(agent/set-model conv-id model-id)` | nil | switches the model for subsequent turns |

`set-model` takes effect on the next `(agent/send …)`. It does not
interrupt an in-flight turn. Bumps `generation`.

### UI

In the AgentBuffer header:

```
┌─ digitone (instrument)  [model: sonnet-4-6 ▾]  status: idle ─┐
```

Standard dropdown widget bound to `(agent/models)` for options and
`(agent/set-model …)` for the change handler. Disabled (greyed) while
status is `Streaming | Compiling | Auditioning` to avoid mid-turn confusion.

### Persistence

Model choice is part of conversation state, so it persists for the lifetime
of the conversation. When V2 adds on-disk conversation persistence, model
choice rides along automatically.

## Persistence

V1: conversations are **in-memory only**. Closing the app loses them.
Drafts that have been Accepted are persisted via the normal
instrument/effect save path.

V2 candidates: persist conversations to disk per-project (`projects/<name>/agent/<conv-id>.json`).

## Cancellation Semantics

`(agent/cancel conv-id)`:
- Drops the conversation's `JoinHandle`. reqwest aborts in-flight HTTP on
  drop; tokio aborts the task.
- Sets `status = Cancelled`.
- Does NOT discard the draft. User can still Accept the last successful
  draft if desired.
- Closing the conversation (`agent/close`) also cancels.

## Error Surfacing

All errors flow through the message stream as system messages. There is no
side-channel error pipe.

- Network / Responses API errors: `system> request failed: <message>`,
  status = `Error`.
- Compile errors after retry budget exhausted: status = `Error`, last system
  message contains the error.
- Audition silent/clipped after retry budget: status = `Error`.

When `status = Error`, the user's next `(agent/send …)` resets state to
`Streaming` and clears `retries_this_turn`.

## System Prompt

Per-kind system prompt embedded in the binary. Contents (roughly):

- Role: "You author dgenlisp instruments/effects for a sequencer DAW."
- Output contract: "Reply with a brief explanation, then a single
  ` ```dgenlisp ` code block containing the complete patch. The block is
  parsed automatically; do not split into multiple blocks. Rewrite the full
  patch each turn — do not produce diffs or partial edits."
- Pipeline awareness: "After your block, the system will compile and
  audition automatically. You will see results as system messages and may
  iterate."
- dgenlisp grammar reference: inline (small) or referenced via
  `search_examples` (if RAG wired).
- Effect-specific addendum (effect mode only): expected signature, sample
  rate handling, etc.

System prompts live in `src/agent/prompts/` as `.md` files included with
`include_str!`.

## File Layout

```
src/agent/
  mod.rs                 — public API, ConversationStore
  store.rs               — state types, snapshots
  task.rs                — per-conversation tokio task, post-turn pipeline
  client.rs              — wraps existing Responses API client
  parse.rs               — fenced block extraction
  audition.rs            — instrument-probe shell-out + parsing
  draft.rs               — DraftSlot type + mount/unmount/accept
  prompts/
    instrument.md
    effect.md
  lisp.rs                — eseqlisp builtin registrations
```

## Implementation Order

1. `ConversationStore` + state types + cheap snapshot readers (no async yet).
2. Lisp builtin surface wired to the store, returning empty/dummy data.
3. `task.rs` skeleton: spawns task on `send`, streams text into messages,
   no pipeline.
4. Responses API client wiring.
5. Fenced-block parser + draft mount.
6. Compile integration.
7. Audition integration (extend `instrument-probe` for effects).
8. Post-turn pipeline with retry budget.
9. UI: AgentBuffer center-pane mode.
10. UI: draft slot rendering in bottom pane + Save/Discard buttons.
11. Entry points (keybinding + instrument picker integration).
12. System prompts + dgenlisp examples.
13. (Optional) `search_examples` tool if a corpus is built.

## Open Questions

- Keybinding for AgentBuffer entry — `Ctrl+G`? Other?
- Should Accept require choosing the destination slot up front, or default
  to the currently selected track?
- Whether to surface a "regenerate from scratch" affordance distinct from
  just sending another message.
- Whether system messages should count toward Responses API token budget
  the same as assistant messages, or get folded/summarized after N turns.
