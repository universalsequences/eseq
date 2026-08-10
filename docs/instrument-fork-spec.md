# Instrument / Effect Fork

Status: design, unbuilt
Owner: alec
Related: `docs/patch-vs-code-editor-spec.md`, `docs/instrument-swap-spec.md`,
`MACRO_MAPPING_SPEC.md`

## 1. Problem

You have an instrument that a song depends on. You want to change it — not to
fix it, but to explore. Today the only way to hear the change is to save it,
and saving overwrites the instrument every project shares.

The failure is worse than "my other song sounds different". Projects address
instrument parameters **positionally**:

```rust
// crates/sequencer/src/project.rs:1384
param_index: usize,
```

Adding a `(param ...)` line anywhere but the end, deleting one, or reordering
two shifts every index after it. Saved p-locks, macro mappings, and automation
in every project that references the instrument keep their old indices and
silently point at different parameters. Nothing errors. The project loads, and
the filter cutoff lane is now driving feedback amount.

Preset banks do not share this flaw — `<name>.presets` keys by parameter name
(`"algorithm": 10.0`) — so the two halves of an instrument's saved state
disagree about what identifies a parameter. Renaming a param breaks presets;
reordering breaks projects.

There is no in-place edit that is safe to explore with.

## 2. Precedent

This exact tension is already solved one level down. Editing a library macro
inside a patch offers two outcomes (`patcher/mod.rs:171`):

```rust
pub enum MacroLibraryActionKind {
    SaveToLibrary,  // write back; every user of the macro sees it
    Fork,           // detach a local copy; the library is untouched
}
```

Instruments and effects have the same shared-definition problem and only the
first option. This spec adds the second, using the same word.

## 3. Design

### 3.1 Fork is the existing draft flow with a different seed

`enter-new-instrument-editor` (`ui/host_commands/instrument_authoring.rs:44`)
already does nearly all of the work:

1. `create_new_instrument_draft_dir()` mints
   `$TMPDIR/eseq-instrument-drafts/draft-<pid>-<stamp>/`
2. writes `NEW_INSTRUMENT_STARTER_DSP` to `dsp.lisp` in it
3. spins up a transient track named `new-instrument-draft/` bound to that dir
4. opens the patch editor over the draft path in
   `InstrumentEditMode::CreateDraft`
5. `save-new-instrument` slugs the typed name, refuses to overwrite an existing
   `instruments/<slug>/`, and materializes the draft there

Fork changes exactly step 2: **seed the draft directory by copying an existing
instrument instead of writing the starter template.** Everything downstream —
live preview on a transient track, error gating via `visible_revision_valid`,
name collision refusal, finalize — is reused unmodified.

Because projects reference instruments by name string
(`instrument_name: "emulations/prophet-5"`, `project.rs:1139`) and finalize
refuses to write over an existing directory, a fork is structurally incapable
of touching its source.

### 3.2 New host commands

```
enter-fork-instrument-editor   { source: "emulations/prophet-5" }
enter-fork-effect-editor       { source: "lexilush" }
```

Both live in `instrument_authoring.rs` alongside their `new-` and `edit-`
siblings and reuse `save-new-instrument` / `save-new-effect` to finalize. The
existing guard at the top of `enter-new-instrument-editor` ("Close the current
editor before creating a new instrument") applies unchanged.

### 3.3 What gets copied

Copying `dsp.lisp` alone produces a broken fork. The full set:

| Artifact | Action | Why |
|---|---|---|
| `dsp.lisp` | copy | the patch |
| `dsp.layout.json` | **copy — mandatory** | without the authored sidecar the patch auto-materializes a fresh layout on open and the node graph is scrambled (see `docs/patch-vs-code-editor-spec.md`) |
| `ui.lisp` | copy if present | custom UI |
| `instrument.json` | copy if present | carries `run_mode` (`free_patch` vs `instrument`) |
| `waves/`, other asset dirs | copy recursively | `triton`, `wavetable` ship sample data |
| `<name>.presets` | copy **and rewrite** | sibling JSON, not inside the dir; its `engine_name` and `source_file` fields embed the old name and must be repointed at the fork |

The presets rewrite is the one non-mechanical step. Everything else is a
directory copy.

Copying presets is the default because the sounds are usually the reason for
the fork. Params are identical at fork time, so the bank is valid by
construction.

### 3.4 Naming

The name field starts **empty**, exactly as in the create flow. No prefill, no
`<source>-2` suggestion.

A prefilled name is a default you can Enter through, and the resulting library
is `prophet-5-2`, `prophet-5-2-2` — names that record where a patch came from
but not what it is. Typing the name is also the moment you decide what the fork
*is*, which is the whole reason you forked. Requiring it costs one interaction
and prevents a class of junk.

This also means Fork adds no naming UI at all: it reuses the create flow's
field, `normalize_patch_name` slugging, and finalize collision refusal
unchanged. The only thing distinguishing a fork from a create, from the name
field's point of view, is what the draft directory was seeded with.

Empty name on save is already rejected ("Name cannot be empty",
`instrument_authoring.rs:330`), so the enforcement path exists.

### 3.5 Entry points

- **Browser context menu** on an instrument or effect → "Fork…". Works without
  loading the thing onto a track.
- **Editor action — a Fork button in the same panel as the finalize button.**
  This is the important one: it catches you mid-exploration, when you started
  editing in place and now want out without discarding your work.

  Concretely, it belongs in the `h-stack` at `crates/sequencer/ui/browser.lisp`
  (the "Save button" block, ~line 1333) that today renders
  Finalize / Save & Add / Save plus cancel. Fork renders in that stack when
  `SEQ.editor-mode` is `edit-instrument` or `edit-effect` — i.e. exactly when
  the primary button means `update-instrument`, the clobbering path. The
  destructive action and its safe alternative sit side by side, which is the
  point: you should not have to leave the buffer, or know a menu exists, to
  avoid overwriting a shared instrument.

  It hides in `new-instrument` / `new-effect` mode (nothing to fork from yet)
  and while `sbrowser-editor-busy?`, matching the existing stack's behaviour.

  Implementation: copy the current *in-editor* source into a fresh draft dir
  rather than re-reading from disk — the whole value is preserving edits you
  have already made — then swap the session's `mode` from `EditExisting` to
  `CreateDraft` and flip `SEQ.editor-mode` to `new-instrument`. That mode flip
  does the rest of the UI for free: the name input at browser.lisp:1227 appears
  (empty, per §3.4) and the primary button relabels itself to Finalize. No new
  panel, no new naming UI — the session simply becomes the create flow it
  should have been, carrying your edits.
- **Cmd+Shift+I** as "fork the current track's instrument", next to Cmd+I for
  new.

### 3.6 Track rebinding

Forking from a live track auto-swaps that track to the fork on finalize, via
the existing swap-instrument path (rebind-not-reload,
`docs/instrument-swap-spec.md`). This makes "let me mess with this" a single
action with the original untouched. Forking from the browser does not touch any
track.

## 4. The guard (the half fork does not solve)

Fork is opt-in, and the damage happens when you forget. Pair it with a check on
save of an **existing** instrument (`update-instrument` / `update-effect`):

1. Parse the `(param ...)` sequence from the source on disk and from the source
   about to be written.
2. Compare as ordered lists of names. A pure append is safe. An insert,
   delete, rename, or reorder is **index-breaking**.
3. If index-breaking, block the save with a three-way choice:
   - **Fork instead** — hands the edit to §3.5's editor Fork action, original
     untouched. Default.
   - **Save anyway** — proceeds, with the count of affected references named
     explicitly in the prompt.
   - **Cancel**

Counting affected references honestly is the fiddly part: the loaded project is
cheap to scan, other project files on disk are not. First cut should say what
it actually knows ("this instrument is used by 3 tracks in the open project;
other projects may also reference it") rather than implying a global count it
did not compute.

A rename-only change additionally invalidates preset banks, which key by name.
Worth calling out separately in the same prompt.

## 5. Build order

1. **Copy helper** — `fork_instrument_files(source_dir, draft_dir)` plus the
   presets rewrite. Pure filesystem, unit-testable against the real
   `instruments/` tree (fork `core/triton`, assert `waves/` and the rewritten
   `.presets` land).
2. **`enter-fork-instrument-editor`** wired to the browser context menu.
   Smallest end-to-end slice: fork → edit → save under a new name → both
   instruments load.
3. **Effect parity** — `enter-fork-effect-editor`, same helper against
   `effects/`.
4. **Editor Fork button** — the browser.lisp button in the finalize stack plus
   the `EditExisting` → `CreateDraft` session conversion behind it. More
   delicate than 1–3 because it mutates a live session with a transient track
   already bound, mid-edit. Arguably the most valuable slice of the feature and
   deliberately not first: it is much easier to build once forking from a cold
   start (2) is known to work.
5. **Param-drift guard** — §4. Independently useful; ship after fork exists so
   "Fork instead" has somewhere to go.

## 6. Open questions

- Should a fork record its ancestor (`"forked_from": "emulations/prophet-5"` in
  `instrument.json`)? Cheap to write, and makes "what did I derive this from"
  answerable later. No consumer today.
- Category placement: same folder as the source (assumed above), or a
  `wips/` staging area? `wips/` already exists as a convention.
- Does the param-drift guard belong on the agent-driven authoring path too, or
  only on interactive save?
