# Program Generators Spec

Epic: eseq-ep8h (slices eseq-ep8h.1–.7, listed in §8). Supersedes the
modal-only plumbing from eseq-k0m2.9 (drafts) and
eseq-k0m2.1 (`eseq modal emit`).

## Goal

A script whose last value is a **program** — data that knows how to turn
itself into DGenLisp — becomes an instrument the same way whatever produced
it. Modal synthesis (`eseq.modal`) is the first generator, not the shape of
the pipeline. A second generator (additive, FM stacks, waveguides, a sampler
layout, an effect chain) needs no Rust, and gets drafts, hot recompiling,
save-as-instrument, the CLI, note-following values, cheap modulation and an
auto-generated panel for free.

## What is modal today, and what only looks modal

| Piece | Lines | Really modal? |
|---|---|---|
| `modal_fe/` (mesher, FE, eigen solve) | 2,865 | yes: a numeric library, stays Rust |
| `modal_plates.rs` (Bessel plate modes) | 214 | yes: stays Rust |
| modal natives in `modal_patch.rs` (`modal-solve`, `modal-read-data`, `modal-nearest-point`, `modal-circ-plate-modes`, modes-file parsing) | ~300 | yes: stays Rust, as a library Lisp calls |
| `generate_modal_instrument`, `ModalScriptHost`, `PATCH_SLOT` | ~100 | no: eval a script, hand its value to a generator, read back source |
| `ModalInstrument::write_to` | ~50 | no, but sniffs the dsp text for `(param out_voice_mode` and the ui text for `:src "….modes.json"` |
| `is_modal_patch_script` | 10 | no: an import sniff deciding C-c C-c routing |
| `ui/host_commands/modal_drafts.rs` | 447 | no: all generic, modal by name (`modal-draft/` prefix, `modal.lisp`, `eseq.modal.save`) |
| `content/ui/modal/save.lisp`, `modal-draft?` in instrument-panel.lisp | ~80 | no |
| `content/ui/modal.lisp` | 4,347 | about two thirds; see §5 |

So about 3,400 lines of Rust are genuinely modal (and correctly native), and
about 600 are a generic pipeline wearing modal names.

## 1. The protocol

### 1.1 A program value

A program is any map with a callable `:generator`:

```lisp
(dict :generator f      ; (f program) -> a result map (§1.2)
      :name "my-synth"  ; optional; the generator's result names it
      ...)              ; anything else is the generator's own data
```

`f` is an ordinary closure stored in the map (eseqlisp calls function values
out of maps: `((get p :generator) p)`). `m/patch` returns its patch dict
with `:generator eseq.modal/generate`; nothing else about patches changes.

Why a closure, not a symbol naming the generator: a user's generator can be a
local function in their script, with no module or registration, and a
generator can wrap another (`(merge base :generator (my-wrapper base))`).

### 1.2 The result map

`(eseq.gen/generate program)` calls the generator and checks what comes back:

```lisp
(dict :kind :instrument          ; only :instrument for now (§7)
      :name "my-synth"           ; folder / engine name, required
      :dsp "…dsp.lisp source…"   ; required
      :ui "…ui.lisp source…"     ; optional: no :ui -> the stock generic panel
      :voice-mode 'out_voice_mode ; optional: the param the host binds to
                                 ;   its voice mode (instrument.json
                                 ;   voice_controls)
      :assets (list "/abs/a.modes.json")) ; optional: files ui.lisp reads by
                                 ;   absolute path, copied beside it on save
;; or
(dict :errors (list "spring bridge: no object bdy" ...))
```

`eseq.gen/generate` returns `(dict :errors …)` itself when the program has no
callable `:generator`, or when the result lacks `:name`/`:dsp` or has a
`:kind` other than `:instrument`, so Rust reads one shape.

The two text sniffs go away: `:voice-mode` replaces searching the dsp for
`(param out_voice_mode `, and `:assets` replaces searching ui.lisp for
`:src "….modes.json"`. Rust still rewrites each asset's absolute path in
ui.lisp to the copy's path (the asset list says which strings those are).

### 1.3 Rust side

New `lisp_host/eseq/generator.rs` (generic), with `modal_patch.rs` reduced
to the modal natives and their tests:

```rust
pub struct GeneratedInstrument {
    pub name: String,
    pub dsp: String,
    pub ui: Option<String>,
    pub voice_mode: Option<String>,
    pub assets: Vec<PathBuf>,
}
impl GeneratedInstrument { pub fn write_to(&self, dir: &Path) -> Result<(), String> }

pub trait ScriptHost { fn eval_at(..); fn set_global(..); }   // was ModalScriptHost
pub fn is_program(value: &Value) -> bool;                     // a map with a callable :generator
pub fn generate_instrument(host, path, script) -> Result<GeneratedInstrument, Vec<String>>;
pub fn current_script_dir() -> Option<PathBuf>;               // was modal's SCRIPT_DIR
```

`generate_instrument` evaluates the script (with the script dir set, so
relative data files resolve), checks `is_program` on the last value, parks it
in a host global and evaluates `(eseq.gen/generate <slot>)`. Natives that read
files relative to the script (`modal-read-data`) call `current_script_dir`.

`write_to` with no `:ui` writes no ui.lisp; the instrument then gets the
stock panel the way any dsp-only user instrument does.

## 2. Routing C-c C-c by result

Today a buffer is a modal script when its text contains `(import eseq.modal`.
Instead: C-c C-c in an eseqlisp buffer that is not a code edit session's own
buffer and is a *script* (its first form is not `(module …)`; module files are
libraries, and evaluating one re-registers a live module) is evaluated, and
**its value decides**:

- a program → generate, then create or update the buffer's draft track (§3);
- anything else → status line "Evaluated: <value>" (C-c C-c on a plain
  script was a no-op before; now it does what eval-buffer does and says so).

The buffer is evaluated once either way. The patch toolbar's Eval button and
the agent pane send the same command with `:session true`: they always mean
the open code session, never a focused script. A generator's errors and an
evaluation error are shown exactly as today.

## 3. Generic drafts

`ui/host_commands/modal_drafts.rs` becomes `drafts.rs` with modal names
removed:

| Today | After |
|---|---|
| `ModalDraft`, `PendingModalCompile` | `Draft`, `PendingDraftCompile` |
| engine name `modal-draft/<name>` | `draft/<name>` |
| `modal.lisp` (the script copy in the draft dir) | `source.lisp` |
| commands `modal-draft-save-open`, `modal-draft-save` | `draft-save-open`, `draft-save` |
| `eseq.modal.save` (content/ui/modal/save.lisp) | `eseq.draft-save` (content/ui/draft-save.lisp) |
| `modal-draft?`, `modal-draft-save-button` in instrument-panel.lisp | `draft?`, `draft-save-button` |
| `sessions.modal_drafts`, `pending_modal_compiles` | `sessions.drafts`, `pending_draft_compiles` |

A draft records the generated `voice_mode` and `assets` alongside its `ui`,
so Save writes the same instrument.json and assets as the CLI would. Today
Save rebuilds a `ModalInstrument` from the engine's dsp alone, which is why
the voice-mode sniff was there.

Drafts are transient tracks and are not saved in projects, so the
engine-name change needs no migration.

## 4. CLI

`eseq generate SCRIPT [DIR]` replaces `eseq modal emit` (kept as an alias that
prints the new name). `eseq modal solve` stays: it is the FE solver's own
command.

## 5. Splitting the Lisp

`content/ui/modal.lisp` mixes physics with general tools for building
instruments as data. The general part moves to `eseq.gen`
(`content/ui/gen.lisp`), and `eseq.modal` imports it. **Every one of the 27
example instruments must emit byte-identical dsp.lisp and ui.lisp before and
after each move**; that is the test for every slice of §5.

### 5.1 `eseq.gen` — protocol, forms, params, voice

- the protocol: `generate` (§1.2) and `program` (`(gen/program f &rest kv)`,
  a convenience building the map);
- list and form helpers: `concat-all`, `sum-form`, `index-of`, `member?`,
  `any?`, `all?`, `head?`, `head-of-3?`, `def-form?`, `form-atoms`,
  `def-names`, `map-atoms`, `rename-in`, `defines?`, `sig-figs`;
- param specs: `pspec`, `param-form`, `spec-ref`, `host-name`, `modulatable?`
  (every range param but the Voicing), the Voicing param itself
  (`voice-mode-params`) and the result's `:voice-mode` for it
  (`voice-mode`, naming `voice-mode-sym`);
- note-following values: `by-note` and its interpolation, `by-note-forms`;
- the voice preamble: `mod-sources`, `preamble` (gate, pitch, velocity,
  trigger, clock, mod1..4, hit, age, hit_velocity, f0);
- event clocks: `change-tick-forms`.

### 5.2 `eseq.gen.modulate` — the modulation pass

`mod-pass` and its analysis (`setup-names`, `event-cones`, `clock-held`,
`held-value-forms`, `clock-form?`, …). Its one modal-specific input becomes
an argument: `:stateful`, the library defmacros that keep state
(`pluck-contact`, `voice-amp`). The generic per-sample heads (`in`,
`read-history`, `svf`, `adsr`, …) and the slow names (`f0`, which the shared
preamble defines) stay in the module.

### 5.3 `eseq.gen.panel` — panel building blocks

The `mx-*` helpers (`ui-helpers`), `control-form`, `num-form`,
`panel-form`, `page-form`, `pages-from`, so another generator can emit a
sectioned panel without copying them, plus `screen-form` (title, optional
drawing, the selected section's page, tabs) and `sections-ui-forms` (a
whole ui.lisp from a list of sections). eseq.modal's screen is `screen-form`
with its patch drawing; the drawing (`link-row`, `mic-rows`,
`drawing-form`) and mesh views stay in `eseq.modal`.
Names keep their `mx-` prefix in the emitted ui.lisp so existing output is
unchanged.

### 5.4 What stays in `eseq.modal`

Objects, connections, air, drivers, data objects, the check, per-object DSP,
strokes, listening, the amp, the program assembly (`patch-forms`), the patch
drawing, mesh views, and `generate` for patches.

## 6. Proof: a second generator

A documented generator with no modal code:
tools/gen/examples/additive.lisp, an additive organ (`:partials 16 :tilt -6`
and a note-following brightness), built from `eseq.gen` alone,
generated through the same Rust entry point, its dsp compiled and its panel
built. It proves the protocol takes non-modal programs and that the split
left `eseq.gen` usable on its own. It also goes in tools/gen/README.md as the
"write your own generator" walkthrough.

## 7. Out of scope (later)

- `:kind :effect`: the same draft flow for effects (draft on a track's fx
  chain). The result shape and the `:kind` check leave room for it.
- MIDI-effect or process generators.
- Generators that return several instruments (a kit).

## 8. Slices

1. **Protocol** (§1, §4): generator.rs, `GeneratedInstrument` with
   `voice_mode` and `assets`, `eseq.gen/generate`, `m/patch` tags itself,
   `eseq generate`. modal_patch.rs keeps only natives. Tests: the existing
   modal tests through the new entry point; a non-program script is rejected;
   a malformed result is rejected by name.
2. **Routing by result** (§2).
3. **Generic drafts** (§3), including the save dialog and instrument-panel
   rename and their tests.
4. **`eseq.gen` core split** (§5.1), byte-identical output.
5. **`eseq.gen.modulate` split** (§5.2), byte-identical output.
6. **`eseq.gen.panel` split** (§5.3), byte-identical output.
7. **Second generator + docs** (§6): the additive generator test,
   tools/gen/README.md, and tools/modal/README.md pointing at it.
