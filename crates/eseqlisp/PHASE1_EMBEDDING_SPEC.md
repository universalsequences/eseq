# ESeqlisp Phase 1 Embedding Spec

## Goal

Turn `eseqlisp` from a standalone experimental editor into an embeddable library that can be hosted by another application, starting with the `sequencer` app.

Phase 1 should enable:

- replacing the external `$EDITOR` / `vim` flow currently used by `sequencer`
- opening a Lisp editor session from a host app
- editing a file-backed buffer or scratch buffer
- evaluating Lisp in that editor
- registering host-provided native functions
- sending asynchronous host commands from Lisp/editor to the host app
- receiving asynchronous status/result events back from the host
- basic host-driven pattern editing commands for the sequencer use case

This phase is explicitly about embedding and host integration, not about making all sequencer UI surfaces Lisp-defined.

## Non-Goals

Do not implement these in phase 1:

- a Lisp-defined sidebar UI for the sequencer
- a Lisp-defined Cirklon grid editor
- direct DSP compilation/execution inside `eseqlisp`
- replacing DGenLisp
- realtime graph mutation directly from the Lisp VM
- a general object system for passing arbitrary Rust structs into Lisp
- multi-window Emacs-like editor architecture
- LLM/agent orchestration beyond the host command abstraction

## Product Direction

`eseqlisp` is the control/UI Lisp and editor runtime.

`DGenLisp` remains the DSP/instrument/effect language and compiler.

The integration model is:

- `eseqlisp` provides editor, VM, keybinding, buffer, eval, and embedding APIs
- the host app provides synchronous native functions and asynchronous host commands
- the host app owns its own state, file conventions, compile jobs, graph mutation, status, and persistence

For the `sequencer` host, phase 1 should support:

- editing `instruments/*.lisp` and `effects/*.lisp`
- triggering compile requests from the embedded editor
- showing compile progress and results
- simple pattern-edit commands from Lisp

## Architecture Overview

Refactor `eseqlisp` into:

- a reusable library crate
- a thin standalone binary that uses the library

Suggested crate structure:

- `src/lib.rs`
- `src/runtime.rs`
- `src/editor.rs`
- `src/buffer.rs`
- `src/host.rs`
- `src/events.rs`
- `src/commands.rs`
- `src/parser.rs`
- `src/compiler.rs`
- `src/vm.rs`
- `src/tui.rs`
- `src/bin/eseqlisp.rs`

The old `main.rs` behavior should move into `src/bin/eseqlisp.rs` or equivalent thin launcher code.

## Core Design Constraints

### 1. Host-agnostic library

The library must not depend on the sequencer app or any sequencer-specific types.

No imports from the host project.

No assumptions about instruments, patterns, tracks, or audio.

Everything host-specific must be injected through registration APIs and command/event channels.

### 2. Clear sync vs async boundary

There are two kinds of host integration:

- synchronous native functions
- asynchronous host commands

Use synchronous native functions only for cheap operations.

Use asynchronous host commands for anything slow or stateful, including:

- instrument/effect compilation
- file generation workflows
- agent/LLM requests
- anything that may take more than a frame or two

### 3. No direct host struct exposure

Do not try to expose raw Rust app structs directly to Lisp in phase 1.

Instead, communicate through:

- scalar values
- strings
- lists
- maps
- explicit native functions
- explicit host commands

This keeps ownership and mutation boundaries manageable.

### 4. Host owns side effects

The host owns:

- filesystem semantics
- compile jobs
- save conventions
- graph mutation
- UI mode transitions in the host app
- app state mutation safety

`eseqlisp` may request an action, but it must not assume the action has completed until the host reports back.

## Public API Requirements

## Library Entry Points

Expose a clean top-level API from `lib.rs`.

The exact names may differ, but the library must provide equivalents to:

```rust
pub struct Runtime;
pub struct Editor;
pub struct EditorConfig;
pub struct SessionConfig;
pub struct HostCommand;
pub struct HostEvent;
pub struct NativeContext;
pub enum Value;
```

## Runtime

The runtime is the Lisp VM plus registered host integration hooks.

Required capabilities:

- construct a runtime
- register synchronous native functions by name
- optionally preload init Lisp
- evaluate strings in persistent VM context
- expose a way for the editor to access the runtime

Suggested API shape:

```rust
impl Runtime {
    pub fn new() -> Self;
    pub fn with_init_source(init: impl Into<String>) -> Self;
    pub fn register_native<F>(&mut self, name: &str, f: F)
    where
        F: Fn(Vec<Value>, &mut NativeContext) -> Result<Value, RuntimeError> + 'static;
    pub fn eval_str(&mut self, src: &str) -> Result<Option<Value>, RuntimeError>;
}
```

Requirements:

- native functions must have access to contextual editor/session state via `NativeContext`
- native functions must be able to enqueue host commands
- native functions must be able to emit editor status messages

## Editor

The editor is the interactive text UI surface and session owner.

Required capabilities:

- create an editor from a runtime
- open scratch buffers
- open file-backed buffers
- save buffers
- run an interactive session
- allow host to push asynchronous events into the editor while it is running
- return a structured session result on exit

Suggested API shape:

```rust
impl Editor {
    pub fn new(runtime: Runtime, config: EditorConfig) -> Self;
    pub fn open_scratch_buffer(&mut self, name: &str, initial: &str) -> BufferId;
    pub fn open_file_buffer(&mut self, path: impl Into<PathBuf>) -> Result<BufferId, EditorError>;
    pub fn open_or_create_file_buffer(
        &mut self,
        path: impl Into<PathBuf>,
        initial: &str,
    ) -> Result<BufferId, EditorError>;
    pub fn set_active_buffer(&mut self, id: BufferId);
    pub fn handle_host_event(&mut self, event: HostEvent);
    pub fn drain_host_commands(&mut self) -> Vec<HostCommand>;
    pub fn run_embedded(&mut self) -> Result<EditorExit, EditorError>;
}
```

`run_embedded()` may own its own event loop internally in phase 1.

## Session Result

The editor must return structured output when it exits.

Suggested shape:

```rust
pub enum EditorExit {
    Cancelled,
    Closed,
    SavedAndClosed,
}
```

Also provide host access to final buffer contents and dirty state.

## Value Model

Use the existing `Value` model as a base, but make it library-safe and host-usable.

Requirements:

- `Number`
- `Bool`
- `String`
- `Keyword`
- `List`
- `Map`
- `Nil` or equivalent falsey empty value

`Symbol`, closures, and functions can remain internal VM concepts if desired.

If keeping current `Value`, ensure host-facing conversions are ergonomic.

Provide helper conversions where practical:

- Rust string <-> `Value::String`
- Rust bool <-> `Value::Bool`
- Rust numbers <-> `Value::Number`
- Vec -> `Value::List`
- key/value map -> `Value::Map`

## Native Functions

## Definition

Synchronous native functions are immediate host-provided builtins callable from Lisp.

They are for cheap operations only.

Examples:

- `(current-buffer-text)`
- `(current-buffer-path)`
- `(load-buffer)`
- `(save-buffer)`
- `(status "Compiling...")`
- `(seq-current-track)`
- `(seq-toggle-step 1 4)`

## NativeContext

Each native function must receive a mutable context object that exposes:

- current editor state snapshot
- current buffer identity
- ability to read current buffer text
- ability to set minibuffer/status text
- ability to enqueue asynchronous host commands

Suggested API shape:

```rust
pub struct NativeContext<'a> {
    pub editor: &'a mut EditorStateView,
    pub commands: &'a mut Vec<HostCommand>,
}
```

The exact type names can differ. The key requirement is that native functions can inspect session state and request host work.

## Required built-in editor natives

Phase 1 must include built-ins or equivalents for:

- `bind-key`
- `s-expression-at-cursor`
- `eval-buffer`
- `eval-selection-or-sexp`
- `current-buffer-text`
- `current-buffer-name`
- `current-buffer-path`
- `save-buffer`
- `load-buffer`
- `save-buffer-as`
- `status`

Behavior requirements:

- `save-buffer` writes the active file-backed buffer immediately
- `load-buffer` reloads the active file-backed buffer from disk and discards unsaved changes
- `save-buffer-as` changes the buffer path and writes it
- `status` updates the minibuffer/status line

## Host Commands

## Definition

Host commands are asynchronous requests sent from the editor/VM to the embedding host.

Examples:

- compile instrument
- compile effect
- ask agent
- create file from template

These commands must not be executed by `eseqlisp`.

They are requests only.

## Command API

Provide a structured enum rather than only a stringly-typed escape hatch.

Required:

```rust
pub enum HostCommand {
    Custom {
        name: String,
        payload: Value,
    },
}
```

Preferred:

```rust
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
```

Phase 1 should implement the preferred version.

## Host Events

The host must be able to push events back into the running editor.

Required:

```rust
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
```

Requirements:

- the editor must render status/error messages visibly
- compile success/failure must be reflected in the minibuffer/status line
- host events must be safe to process during the editor loop

## Editor UX Requirements

## Phase 1 Session Model

The initial host integration can remain modal/fullscreen:

- host app suspends its own TUI
- host runs embedded `eseqlisp` editor session
- editor exits
- host resumes its own TUI

This is acceptable for phase 1.

Do not attempt split-screen embedded rendering inside the host app yet.

## Buffer Features

Phase 1 buffer requirements:

- scratch buffer support
- file-backed buffer support
- dirty tracking
- save
- save as
- load file
- set initial text
- path metadata

Preferred but optional:

- multiple buffers
- switch buffer command

If multiple buffers are too much for phase 1, it is acceptable to support one active buffer plus a scratch buffer, but file-backed editing must work.

## Keybindings

Keep the current Lisp-configurable keybinding model and expand it enough for phase 1.

Required defaults:

- `C-q` quit
- `C-s` save buffer
- `C-x C-s` save buffer
- `C-x C-c` quit
- `C-x C-e` eval s-expression
- a key for eval buffer

Required host-command bindings for the sequencer use case:

- a keybinding to request `compile-instrument`
- a keybinding to request `compile-effect`

These may be implemented in `init.lisp` using built-ins and host-command helpers.

## Rendering

The TUI can remain simple in phase 1.

Required:

- text area
- cursor
- matching paren highlight
- status line / minibuffer
- dirty marker
- current buffer name
- current file path if available

No need for syntax highlighting in phase 1.

## Required Lisp API for Host Commands

Provide Lisp-callable helpers for host command submission.

Required forms:

- `(host-command name payload)`
- or explicit wrappers like `(compile-instrument)` built on top of host command submission

Recommended implementation:

- low-level primitive: `(host-command "compile-instrument" payload-map)`
- convenience wrappers in init Lisp or built-in registration

Examples:

```lisp
(host-command "compile-instrument"
  (dict :source (current-buffer-text)
        :path (current-buffer-path)))
```

Convenience wrapper:

```lisp
(def compile-instrument ()
  (status "Compiling instrument...")
  (host-command "compile-instrument"
    (dict :source (current-buffer-text)
          :path (current-buffer-path))))
```

## Sequencer Host Integration Contract

This section describes what the host app will eventually provide. `eseqlisp` should not implement these behaviors itself, but must support the API shape.

## Host Commands required for Phase 1 sequencer integration

The sequencer host is expected to implement:

- `compile-instrument`
- `compile-effect`

Expected payload shape for `compile-instrument`:

```lisp
(dict
  :source "...buffer text..."
  :path "instruments/foo.lisp" ; optional
  :suggested-name "foo")       ; optional
```

Expected host behavior:

- optionally save or update the source file
- kick off async compile
- when finished, send success/failure event back to editor
- graph mutation, if any, is handled by the host outside `eseqlisp`

The same model applies to `compile-effect`.

## Synchronous sequencer natives required for basic pattern editing

The embedding library should support registering natives so that the sequencer host can provide a minimal command set like:

- `seq-current-track`
- `seq-current-step`
- `seq-num-steps`
- `seq-toggle-step`
- `seq-clear-step`
- `seq-clear-track`
- `seq-set-velocity`
- `seq-set-note`
- `seq-status`

Example target Lisp usage:

```lisp
(seq-toggle-step 1 4)
(seq-clear-track 2)
(seq-set-note 1 7 60)
```

These should be synchronous native calls from the host, not `eseqlisp` features.

## Safety Constraints for Sequencer Host

Although implemented in the host, `eseqlisp` must be designed with the following assumptions:

- some host mutations are cheap and safe synchronously
- some host operations are expensive or timing-sensitive and must be async
- compile requests must not block the editor loop for seconds
- host may reject commands or return errors

Therefore:

- native functions must return errors cleanly
- host command submission must not panic if host is busy
- status/error delivery must work even if compile fails

## Refactor Plan

## 1. Convert crate to library + binary

Required tasks:

- add `src/lib.rs`
- move standalone launch code out of current `main.rs`
- preserve ability to run `cargo run` for local editor testing

Acceptance criteria:

- another Rust crate can depend on `eseqlisp` as a library
- standalone binary still works

## 2. Separate VM/runtime from standalone startup

Required tasks:

- create runtime abstraction around the current VM
- move native registration into reusable runtime APIs
- keep persistent eval context across calls

Acceptance criteria:

- host can instantiate runtime without starting TUI
- host can register extra natives before editor session starts

## 3. Add host command/event channel

Required tasks:

- introduce `HostCommand` and `HostEvent`
- add queueing/draining APIs
- add editor-side event ingestion

Acceptance criteria:

- Lisp code can request a host command
- host can inspect queued commands
- host can push back status/result events during session

## 4. Make buffers file-aware

Required tasks:

- add optional path metadata to buffers
- add load/save/save-as support
- retain dirty tracking

Acceptance criteria:

- open existing file into buffer
- save back to same path
- save to new path

## 5. Improve editor built-ins for phase 1

Required tasks:

- add `current-buffer-text`
- add `current-buffer-path`
- add `save-buffer`
- add `save-buffer-as`
- add `status`
- add a host command primitive

Acceptance criteria:

- `init.lisp` can define a compile command using only built-ins

## 6. Add a session-oriented host API

Required tasks:

- editor can run as an embedded session
- editor returns structured exit state

Acceptance criteria:

- host app can open editor on a file, wait for session end, inspect result

## 7. Preserve standalone usability

Required tasks:

- keep current single-process standalone editor workflow
- default init file still works

Acceptance criteria:

- local development of `eseqlisp` remains easy without the sequencer host

## Detailed Data Model Requirements

## Buffer

Add or support fields equivalent to:

```rust
pub struct Buffer {
    pub id: BufferId,
    pub name: String,
    pub path: Option<PathBuf>,
    pub lines: Vec<String>,
    pub cursor: (usize, usize),
    pub dirty: bool,
    pub scroll_top: usize,
}
```

Requirements:

- buffer id must be stable during session
- file path is optional
- `name` may be derived from path or a scratch name

## Editor state

Add or support:

- active buffer id
- minibuffer/status text
- pending host commands
- pending key chord
- optional recent command status

## Error Handling

Do not use panic-driven control flow for host integration errors.

Required error classes:

- parse/eval error
- file IO error
- buffer-not-file-backed error
- unknown native
- host command submission error

Expose meaningful errors to:

- host application
- minibuffer/status line when appropriate

## Init Lisp Requirements

The init file should remain a place to define commands and keybindings.

Phase 1 init should be able to define helpers like:

```lisp
(def compile-instrument ()
  (status "Compiling instrument...")
  (host-command "compile-instrument"
    (dict :source (current-buffer-text)
          :path (current-buffer-path))))

(def compile-effect ()
  (status "Compiling effect...")
  (host-command "compile-effect"
    (dict :source (current-buffer-text)
          :path (current-buffer-path))))

(bind-key "C-c C-k" "compile-instrument")
(bind-key "C-c C-c" "compile-effect")
```

The exact keys can vary.

## Acceptance Criteria

All of the following should work by the end of phase 1.

### Standalone library acceptance

- `eseqlisp` can be used as a dependency from another Rust crate
- a host can create a runtime and register a native function
- a host can create an editor and open a file-backed buffer

### Editor acceptance

- open a `.lisp` file into the editor
- edit text
- save file
- evaluate sexp under cursor
- show minibuffer/status messages
- exit cleanly with terminal restored

### Host command acceptance

- Lisp can enqueue a host command
- host can drain and inspect that command
- host can send back `Status`, `Error`, and `CompileFinished` events
- editor displays returned messages

### Sequencer-oriented acceptance

The following should be possible for a host like `sequencer`:

- open `instruments/foo.lisp`
- edit it
- trigger `compile-instrument`
- editor immediately remains responsive
- host performs compile asynchronously
- host sends success/failure event back
- editor shows result

And, if the host registers simple pattern natives:

- `(seq-toggle-step 1 4)` mutates the host pattern
- `(seq-clear-track 2)` mutates the host pattern
- `(seq-set-note 1 7 60)` mutates the host pattern

## Suggested Implementation Notes

These are recommendations, not hard constraints.

- keep the current parser/compiler/vm mostly intact for phase 1
- focus changes on library boundaries, context passing, and editor/session APIs
- keep host commands as queued messages rather than callbacks that execute arbitrary host logic in the VM stack
- prefer simple event polling in the editor loop
- do not over-design plugin systems yet

## Explicitly Deferred to Later Phases

- Lisp-defined host rendering surfaces
- mode API for sidebar/cirklon replacement
- persistent background agent workers
- richer editor commands and minibuffer prompt system
- syntax highlighting
- multiple windows/panes
- project-wide host object reflection
- structured undo/redo model

## Deliverables

The implementation in this repo should produce:

- a reusable `eseqlisp` library crate
- a retained standalone `eseqlisp` executable
- public host integration APIs
- file-backed editing support
- native registration support
- host command/event support
- documentation and a small embedding example

## Documentation Required

Add a short embedding guide documenting:

- how to instantiate the runtime
- how to register natives
- how to process host commands
- how to feed host events back into the editor
- how to open a file buffer
- how to run the standalone binary

## Final Instruction to Implementer

When implementing this spec:

- optimize for a clean embedding API over feature breadth
- keep sequencer-specific behavior out of the library
- preserve the current lightweight editor feel
- build the smallest abstraction that cleanly supports host-provided natives and async host commands

The target outcome is not “make a powerful editor.”

The target outcome is:

`eseqlisp` becomes a reusable control Lisp/editor runtime that another app can host safely and incrementally.
