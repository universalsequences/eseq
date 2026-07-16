# Tool-Driven Agent Mode Spec

## Purpose

Replace the current "every assistant response is an instrument revision" agent
flow with a conversational, tool-driven agent that can explain, inspect, create,
edit, validate, and apply DGenLisp instruments and effects through explicit
host tools.

The user should be able to ask read-only questions such as "explain how
minimoog-lad2 works" without triggering a compile or track update. Mutating the
project should require a model-visible tool call whose result tells the model
whether the change succeeded and, if not, what failed.

This spec supersedes the implicit post-turn artifact pipeline described in
`agent-mode-spec.md` for future agent work. Existing implementation can be
reused where it is robust, but the target behavior is explicit tools rather
than mandatory fenced code blocks.

## Current Problems

The current `metal-seq-agent` path is useful but too narrow:

- It uses a text-only model request with no model-visible tools.
- Its instrument prompt requires every assistant answer to include complete
  `dsp.lisp` and `ui.lisp` code blocks.
- The host parses every assistant answer as an instrument artifact.
- A valid artifact is compiled, auditioned, stored as a draft, and currently
  auto-applied by the UI loop.
- A read-only user question can therefore cause an unintended instrument edit.

There is also an older tool-capable agent path in `src/agent/protocol.rs` and
`src/tui/mod.rs`, but it is not the path used by `metal-seq-agent`. That path
already has useful read/doc/edit concepts, but its mutating tools return
pending app actions rather than artifact IDs and explicit apply/validation
semantics.

## Design Goals

- Conversational by default. Plain assistant text is valid and never mutates
  project state.
- Mutations happen only through explicit tools.
- Read-only inspection tools must cover local docs, examples, saved
  instruments, saved effects, and current-track sources.
- Generated code is an artifact with a stable ID before it is applied.
- Applying an artifact performs compile, UI validation where relevant,
  host-side load/init, and signal checks, then returns structured success or
  failure to the model.
- The same architecture supports instruments now and effects soon.
- Existing local examples and the DGen catalog remain the source of truth for
  API usage.
- No partial or silent success. A tool that cannot fully complete its contract
  returns failure with actionable diagnostics.

## Non-Goals

- AST-level patch editing in V1. Tool inputs are complete replacement source
  files.
- Letting the model bypass host validation by directly writing to final
  instrument/effect locations.
- Auto-applying artifacts just because code appeared in an assistant message.
- Hidden mutation from read tools.
- Preserving the old mandatory-code response contract.

## Core Model

### Conversation

A conversation is a normal chat transcript plus a set of artifacts created or
read during that conversation.

```rust
pub enum AgentDomain {
    Instrument,
    Effect,
}

pub enum AgentArtifactKind {
    Instrument,
    Effect,
}

pub enum AgentArtifactStatus {
    Draft,
    Validated,
    Applied,
    Failed,
    Finalized,
}

pub struct AgentArtifact {
    pub id: AgentArtifactId,
    pub kind: AgentArtifactKind,
    pub display_name: String,
    pub dsp_source: String,
    pub ui_source: Option<String>,
    pub created_from: Option<AgentArtifactId>,
    pub status: AgentArtifactStatus,
    pub last_validation: Option<AgentValidationReport>,
    pub applied_target: Option<AgentAppliedTarget>,
}
```

The artifact store should be in Rust, not in model text. Assistant messages may
summarize source changes, but the host should not parse arbitrary fenced code as
an artifact unless it is passed to an artifact creation/update tool.

### Validation Report

```rust
pub struct AgentValidationReport {
    pub ok: bool,
    pub compile_ok: bool,
    pub ui_ok: Option<bool>,
    pub load_ok: bool,
    pub signal_ok: bool,
    pub peak_db: Option<f32>,
    pub rms_db: Option<f32>,
    pub clipped: bool,
    pub diagnostics: Vec<String>,
}
```

For instruments, signal checks should use the same compile/load/init path as
the app. `instrument_probe` remains the fast external check when changing
instrument DSP, tensor/wavetable loading, or host-side instrument init.

For effects, the same validation shape should be used once effect probing
exists. The signal check should feed a known input and verify the output is not
silent, not clipped, and, where appropriate, measurably differs from input.

## Tool Surface

Tool names below are conceptual names. Final names should be stable, explicit,
and provider-friendly.

### Documentation Tools

| Tool | Mutates | Purpose |
|---|---:|---|
| `lookup_dgen_docs` | no | Look up DGenLisp operators, attributes, preamble helpers, and related examples from the local catalog. |
| `list_examples` | no | List indexed local examples by kind: `instrument`, `effect`, or `any`. |
| `read_example` | no | Read a known indexed example source. |

The existing catalog-backed implementations are a good base. The prompt should
tell the model to use these whenever it is uncertain about syntax, operators,
modulation metadata, UI helpers, or effect conventions.

### Source Discovery Tools

| Tool | Mutates | Purpose |
|---|---:|---|
| `list_instruments` | no | List saved instruments, including folder-style instruments with `dsp.lisp`/`ui.lisp`. |
| `read_instrument_source` | no | Read saved instrument source by name/path. Returns `dsp_source`, optional `ui_source`, params, outputs, modulators, and source paths. |
| `read_current_instrument_source` | no | Read the currently selected custom instrument source and UI. |
| `list_effects` | no | List saved effects. |
| `read_effect_source` | no | Read saved effect source by name/path. |
| `read_current_effect_source` | no | Read the selected custom effect slot source. |

`read_instrument_source` is the tool that makes questions like "explain how
minimoog-lad2 works" reliable. The model should call `list_instruments` when it
needs to resolve an ambiguous name, then `read_instrument_source`, then answer
in plain text.

### Artifact Tools

| Tool | Mutates | Purpose |
|---|---:|---|
| `create_instrument_artifact` | yes, draft only | Create a draft artifact from complete `dsp_source` and `ui_source`, then compile/load/UI-validate/probe it before succeeding. Does not apply it to a track. |
| `update_instrument_artifact` | yes, draft only | Replace an existing draft artifact's source, then compile/load/UI-validate/probe it before succeeding. |
| `apply_instrument_artifact` | yes, project | Re-verify an artifact and apply it to a new or existing custom instrument track. |
| `create_effect_artifact` | yes, draft only | Create a draft effect artifact from complete source, then compile/load/probe it before succeeding. |
| `update_effect_artifact` | yes, draft only | Replace an existing draft effect artifact's source, then compile/load/probe it before succeeding. |
| `apply_effect_artifact` | yes, project | Re-verify an effect artifact and apply it to the current track or selected effect slot. |

Artifact tools should return structured IDs and reports, not only prose.
Validation is not model-optional: every create/update tool validates before it
can return success, and every apply tool re-verifies the artifact before project
mutation.

Example successful create response:

```json
{
  "artifact_id": "inst-42",
  "kind": "instrument",
  "status": "validated",
  "summary": "Created validated draft artifact inst-42: glassy-fm-pad.",
  "validation_report": {
    "ok": true,
    "peak_db": -3.2,
    "rms_db": -14.1,
    "clipped": false
  }
}
```

Example failed create/update/apply response:

```json
{
  "artifact_id": "inst-42",
  "ok": false,
  "stage": "ui_validation",
  "diagnostics": [
    "ui.lisp references unknown parameter filter_env_amt. Valid params: cutoff, resonance, filter_env_amount, gain"
  ]
}
```

### Apply Semantics

`apply_instrument_artifact` should accept:

```json
{
  "artifact_id": "inst-42",
  "target": {
    "mode": "new_track"
  }
}
```

or:

```json
{
  "artifact_id": "inst-42",
  "target": {
    "mode": "replace_current_track"
  }
}
```

Apply succeeds only if re-verification and the host load/apply path both
succeed. The model must not claim an artifact was applied unless the apply tool
returns success.

Effects should mirror this:

```json
{
  "artifact_id": "fx-9",
  "target": {
    "mode": "next_free_slot_on_current_track"
  }
}
```

or:

```json
{
  "artifact_id": "fx-9",
  "target": {
    "mode": "replace_current_effect"
  }
}
```

## Prompt Contract

The system prompt should say:

- You may answer questions in plain text.
- Do not create or edit artifacts unless the user asks to create, change,
  refine, apply, save, or audition something.
- Use read-only tools for explanations and analysis.
- Use documentation/example tools before inventing unfamiliar syntax.
- For instruments, generated artifacts must include complete `dsp_source` and
  complete `ui_source`.
- For instrument UI, use the current lego-style UI building blocks:
  `ui-control-block-*`, `ui-readout-block-*`, and `ui-lego-*`.
- For effects, generated artifacts include complete effect source and no synth
  UI unless a later effect UI contract is added.
- To change the project, call an apply tool. Do not claim a change was applied
  unless the apply tool succeeded.
- If a create/update/apply tool fails, use the diagnostic output to revise the
  artifact and try again, within a bounded retry budget.

The prompt must not require code blocks on every turn.

## Turn Flow

### Read-Only Question

```
user: explain how minimoog-lad2 works
  ↓
model calls list_instruments if needed
  ↓
model calls read_instrument_source
  ↓
model optionally calls lookup_dgen_docs for unfamiliar operators
  ↓
assistant answers in prose
```

No artifact is created. No compile or apply happens.

### New Instrument

```
user: make a glassy FM pad
  ↓
model calls lookup_dgen_docs/read_example as needed
  ↓
model calls create_instrument_artifact(dsp_source, ui_source)
  ├─ failure → update artifact and retry within budget
  └─ success → ask whether to apply, or apply immediately if user requested creation on a track
  ↓
model calls apply_instrument_artifact(artifact_id, target)
  ↓
assistant reports what changed
```

### Edit Existing Instrument

```
user: make the current bass less bright
  ↓
model calls read_current_instrument_source
  ↓
model creates or updates an artifact derived from current source
  ├─ failure → revise artifact and retry within budget
  └─ success → continue
  ↓
model applies with replace_current_track
```

### New Effect

```
user: add a widening chorus to this track
  ↓
model reads examples/docs
  ↓
model creates effect artifact
  ├─ failure → revise artifact and retry within budget
  └─ success → continue
  ↓
model applies to next free slot on current track
```

## UI Implications

- The agent panel should show normal chat messages and a compact artifact list.
- Each artifact row should show ID, name, kind, status, validation summary, and
  applied target if any.
- Manual buttons can call the same apply/discard/finalize host paths as the
  tools.
- There should be no automatic apply loop over idle drafts.
- The UI should distinguish read-only tool calls from project-mutating tool
  calls in the transcript.

## Implementation Plan

### Phase 1: Stop Implicit Mutation

- Remove mandatory code-block post-processing from normal chat turns.
- Remove or gate the auto-apply loop for ready drafts.
- Use the tool-capable request path for `metal-seq-agent`.
- Update the system prompt so plain answers are valid.

### Phase 2: Read Tools

- Add `list_instruments`.
- Add `read_instrument_source` that returns both `dsp.lisp` and `ui.lisp` for
  folder-style instruments.
- Add `list_effects` and `read_effect_source`.
- Keep and expose `lookup_dgen_docs`, `list_examples`, and `read_example`.

### Phase 3: Instrument Artifact Tools

- Add an artifact store to the agent conversation state.
- Add `create_instrument_artifact`, `update_instrument_artifact`,
  and `apply_instrument_artifact`.
- Move the existing instrument compile/UI-validate/audition logic into
  create/update/apply so validation is mandatory and not model-selected.
- Move current accept/apply behavior behind `apply_instrument_artifact`.

### Phase 4: Effect Artifact Tools

- Add effect artifact create/update/validate/apply tools.
- Reuse existing effect compile/load paths.
- Add or adapt a probe for effect signal checks.

### Phase 5: Cleanup

- Deprecate `create_instrument_track` and `update_current_instrument` in favor
  of artifact/apply tools.
- Deprecate `apply_effect_to_current_track` and `update_current_effect` in
  favor of effect artifact/apply tools.
- Update docs and tests so tool-driven behavior is the only supported agent
  contract.

## Testing Requirements

- A read-only prompt that asks for explanation must not create, validate, or
  apply an artifact.
- `lookup_dgen_docs` must still find preamble helpers such as `svf`, `ladder`,
  `polyblep_saw`, and `polyblep_pulse`.
- `read_instrument_source` must resolve folder-style and legacy instruments
  unambiguously.
- `create_instrument_artifact` must not write to final instrument storage, and
  must reject unknown UI parameter refs, legacy generated UI helpers, compile
  failures, silent output, and clipped output.
- `update_instrument_artifact` must preserve the previous validated artifact
  source and status if the replacement fails validation.
- `apply_instrument_artifact` must roll back or leave existing track state
  unchanged on save/load/apply failure.
- The UI must not auto-apply an idle draft.
- Effect artifact validation must fail silent, clipped, or unloadable effects
  once effect probing exists.

## Open Questions

- Should the model apply immediately after "make an instrument", or should the
  default be "create + validate artifact, then ask"? A reasonable default is:
  if the user asks to "make/add/create on this track", apply; if they ask to
  "draft/design", stop at validated artifact.
- Should artifacts survive app restart? V1 can keep them in conversation state;
  durable artifact history can come later.
- Should finalized instruments/effects keep a link to their source artifact ID?
  This would make future "explain/change what you just made" turns easier.
- Should validation always include audition/probe, or should expensive signal
  checks be opt-in after compile/UI validation passes? For instruments, keep it
  automatic because the probe is fast and catches silent patches.
