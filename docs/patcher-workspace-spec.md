# Patcher workspace: toolbar, save flow, inspector

Status: rev 1, 2026-10-08. Epic: `eseq-oj8x` (slices `.1`–`.6`).

## 1. Problem

Patcher mode (`instrument-patcher-layout-spec`, `content/ui/seq-layout.lisp:266`)
is a patch canvas with a macro sidebar on the left and a bottom bar of three
buffers: `*samples*` (repurposed as the editor's save panel), `*patch-mixer*`
and `*fx*` (instrument panel + Track FX drop zone).

What is wrong with it:

1. **"Finalize" conflates two actions.** On a new instrument it means "save
   under this name" *and* "leave the patcher and go back to the sequencer".
   Neither half is called finalize anywhere else.
2. **Save on an existing instrument overwrites it in place.** Projects store
   param values positionally against `dsp.lisp`, so an in-place save can
   silently change the sound of every project using it. Fork is the escape
   hatch, but it is opt-in and sits next to the dangerous button.
3. **Instrument vs Free Patch is asked at save time**, inside the save panel,
   although it is a property of the patch (`run_mode` in `instrument.json`)
   that affects how it plays while you are editing it.
4. **Hiding a panel hides Save.** The transport's sidebar toggles drive
   `samples-sidebar-visible`, which in patcher mode removes `*samples*` from
   the bottom bar, which is the only place Save/Finalize lives. The toggle
   does not hide the macro sidebar, which is the sidebar the user is looking at.
5. **The bottom bar wastes most of its width.** The instrument panel has a
   natural width; the remaining ~70% to its right is an empty Track FX drop
   zone that has little purpose while authoring an instrument.

## 2. Layout

```
┌────────────┬──────────────────────────────────────────────────────────────┐
│ macro      │ PATCH TOOLBAR                                                │
│ sidebar    ├──────────────────────────────────────────────────────────────┤
│ (toggle-   │                                                              │
│  able)     │                       PATCH CANVAS                           │
│            │                                                              │
├────────────┴──┬───────────────────────────┬───────────────────────────────┤
│ *patch-mixer* │ instrument panel (natural │ *patch-inspector* (flex)      │
│               │ width)                    │                               │
└───────────────┴───────────────────────────┴───────────────────────────────┘
```

```lisp
(v-stack
  transport
  (h-stack MACRO-SIDEBAR (v-stack PATCH-TOOLBAR PATCH-CANVAS))
  (h-stack PATCH-MIXER INSTRUMENT-PANEL INSPECTOR))
```

- `*samples*` leaves the patcher layout entirely. `editor-header`
  (`content/ui/browser.lisp:2184`) and its patcher-mode branch in
  `build-widgets` are deleted once the toolbar ships.
- The toolbar is a full-width row *above the canvas only*, not above the
  sidebar, so the sidebar keeps its full height.
- The Track FX drop zone leaves the patcher bottom bar. Track FX authoring is a
  sequencer concern; the instrument being edited is previewed dry.
- The code surface (`instrument-patcher-source-layout-spec`) and learn layout
  (`instrument-patcher-learn-layout-spec`) get the same toolbar row so save
  semantics do not depend on surface.

### 2.1 Panel toggles in patcher mode

The three transport icons (`content/ui/transport.lisp:968-985`) keep their
positions but are re-targeted while `seq-layout-mode` is a patcher mode:

| Icon | Sequencer mode | Patcher mode |
|---|---|---|
| 1 (sidebar) | `samples-sidebar-visible` | `patch-macros-panel-visible` (same as `C-x m`) |
| 2 (mixer) | `mixer-panel-visible` | `*patch-mixer*` in the bottom bar |
| 3 (lower) | `lower-panel-visible` | instrument panel + inspector |

Patcher mode gets its own flags (or `seq-toggle-*` dispatches on layout mode);
it must not flip the sequencer's flags, so leaving the patcher restores the
sequencer exactly as it was. Icon highlight reflects the flag that icon
currently drives.

The toolbar is never hidden by any toggle. That is the fix for problem 4:
there is no layout in which Save is unreachable.

## 3. Patch toolbar

```
● binauralfree  v3 ▾ │ [Instrument|Free] │ root / dsp.lisp        • edited   [Patch|Code]   [Save] [Save as…] [Done]
```

Left to right:

- **Name.** An inline text field. On a new, unnamed patch it shows the
  placeholder `untitled` and is the focus target of the first Save. On an
  existing instrument, renaming it plus Save cuts a new release under the new
  name (a release's `name` may differ from its predecessor's, the way Digi
  Drift became Digi Syn).
- **Release menu `vN ▾`** (built: every release with its open-project track
  count, and *Restore vK*, offered while the patch is saved, which cuts a new
  release holding vK's files, plays it on the track and reopens the editor on
  it: `editor-restore-release`. *Open vK* and *Revert as unsaved edits* were
  not built: a retired release is never a write target, and restoring as a
  new release covers going back). Hidden for unsaved and unversioned patches.
  The original design: it
  lists the releases newest first, with the release the track is pinned to
  marked. Entries:
  - *Open vK*: re-pin the edited track to release K and load it into the
    patcher (dirty-check first).
  - *Revert to vK*: load K's source as unsaved edits on top of the current
    release.
  - Count of tracks in the **open project** pinned to each release. Do not
    imply a global count; other project files are not scanned
    (`instrument-fork-spec.md` §4 makes the same point).
- **Run mode `[Instrument|Free]`.** A segmented control bound to `run_mode`.
  It is changeable at any time, takes effect in the preview immediately, and
  is written on Save. It is not a save-time question any more.
- **Breadcrumb.** `root / dsp.lisp [/ macro]`, moved from the Rust canvas
  overlay (`widget_render/patcher/render.rs:264-283`, string from
  `patcher_breadcrumb()` in `patcher/state.rs:1670`) into the toolbar through a
  native that returns the breadcrumb segments. Segments are clickable to pop
  macro levels. The canvas overlay is suppressed when the toolbar is present.
- **Dirty marker** `• edited` when the source differs from the pinned release.
  The editor status and compile errors that `editor-header` showed
  (`editor.error`, "Preview compiling…") also render here, truncated, with the
  full text on hover.
- **Surface toggle `[Patch|Code]`.** It replaces the "View code" and "Open as
  patch" buttons. "Eval (C-c C-c)" stays a code-surface keybinding and is
  shown as a toolbar button only on the code surface.
- **Actions.** Covered in §4.

Macro-editing actions (`editor-macro-action?`, "Save macro" and similar) take
over the action group the same way they took over the finalize stack: while
editing a macro, the primary button is the macro action.

### 3.1 Theming

Like `*transport*`: a borderless tile with no fill of its own, so the window
background shows and every theme matches without new slots. Text uses `:fg`
/ `:dim`, separators `:mixer-strip-border`. (Rev 1 added
`:patch-toolbar-bg/-fg/-border`; a theme that omitted them kept the previous
theme's colors, so they were removed.)

## 4. Save semantics

| Action | Key | New patch | Existing instrument |
|---|---|---|---|
| **Save** | ⌘S | Requires a name, then writes `<user>/instruments/<name>/` and stays in the patcher | Writes the session's release (§5) and stays in the patcher |
| **Save as…** | ⇧⌘S | — | Fork: new lineage under a new name (`fork-editor-session`), and the track is rebound to the fork |
| **Done** | button | Returns to the sequencer, with a dirty prompt | Returns to the sequencer, with a dirty prompt |

- ⌘S / ⇧⌘S are File > Save / Save As, which act on the patch while the editor
  is open (`application-menus.lisp`), as in Max. Done has no key: Esc already
  dismisses the agent bubble and ⌘↩ is the patcher's create-below.
- The dirty prompt is inline in the toolbar's action group (*Unsaved changes:
  Save / Discard / Keep editing*), not a modal, and lapses once nothing is
  unsaved. Save there is `editor-save :done true`.
- Leaving the patcher and saving are independent. Done on a clean patch is
  simply "back".
- **Dirty** (`editor.dirty`): always for a draft; otherwise the evaluated patch
  or the run mode differs from what was saved. Moving boxes alone does not
  count (the layout is written on Save regardless).
- **Cancel goes away.** Its two meanings are covered by Done followed by
  Discard (leave without saving) and by Revert in the release dropdown.
- `save-new-instrument`, `update-instrument`, `fork-editor-session` and
  `cancel-editor` (`ui/host_commands/instrument_authoring.rs`) are kept as the
  primitives. New host commands `editor-save`, `editor-save-as` and
  `editor-done` dispatch on `editor.mode` so the toolbar does not branch on
  mode in Lisp. "Return to the sequencer" is split out of `save-new-instrument`.
- A draft's first Save converts the session in place to an edit of the saved
  instrument: the patcher stays on the draft dir (`path`), Save writes to the
  instrument's home (`save_path`), and the draft dir is deleted on Done.
- Effects follow the same pattern (`save-new-effect` / `update-effect` take
  `stay`): a draft effect's first Save loads it into its slot and the session
  carries on as an edit of it; Done closes a clean one without recompiling.

## 5. User-tier versioning

Versioning is implemented for the factory tier only (`factory_release_roots`,
`lisp_host/dgen/instrument_storage.rs:291`). This spec extends it to user-tier
instruments, with the same manifest and the same `@<release>` pinning
(`docs/instrument-versioning-spec.md`), written by the app instead of by hand.

### 5.1 Layout

Identical to factory:

```
<user>/instruments/binauralfree/
  instrument.json          current, releases
  dsp.lisp ui.lisp dsp.layout.json      current release
  versions/1/ versions/2/               frozen releases
  versions/1.presets ...
```

- `factory_release_roots` grows a user root. The release index cache is
  invalidated on save.
- Legacy single-file user instruments (`<name>.lisp` plus
  `<name>.instrument.json`) are migrated to folder form on their first
  versioned save. Unpinned ids resolve to release 1 through the existing
  name-match rule, so old projects keep loading.
- An unversioned user instrument (no `releases`) becomes versioned on its first
  Save from the patcher: the on-disk source becomes `versions/1` with
  `name` = its current logical path, and the save becomes release 2.
- Storage API: `cut_user_release(source, new_name)`
  (`lisp_host/dgen/instrument_storage.rs`) freezes the top folder into
  `versions/<current>/` (copying, so the new release starts from it, presets
  included), rewrites the manifest, invalidates caches, and returns the new
  pinned `user:<lineage>@<n>` and the top `dsp.lisp` to write. After the first
  cut an unpinned `user:<lineage>` means release 1, so later writes go through
  the pinned id; a retired release's source is never a write target (its
  `versions/<n>.presets` bank stays writable: presets are user data).
- Renames (Save with a new name) set the new release's `name` only; the top
  folder, and so the lineage in every pin, stays put in this rev. The unpinned
  new name resolves to that release through the name-match rule.
- Bare names (`<logical>/`, the browser and runtime spelling) never consult the
  user index: they keep meaning the top folder's current files, and saves pin
  them to the release folder they land on.

### 5.2 When a Save cuts a release

Cutting a release on every ⌘S would produce dozens of releases per session. The
rule:

- **One working release per editing session.** The first Save in a session
  cuts `vN+1` and pins the edited track to it. Later Saves in the same session
  *amend* `vN+1` in place, because only this session's track has been bound to
  it.
- **Except when a project save could have pinned it.** If the project has been
  saved since the working release was cut, the next Save cuts `vN+2` instead of
  amending, because a project file on disk now references `vN+1`.
- **Index-breaking edits are not special-cased.** A release is frozen once
  referenced, so the param-drift guard (`instrument-fork-spec.md` §4) is not
  needed on this path. It remains relevant only for factory instruments edited
  in place by developers.

- A draft saved this session counts as its working release
  (`working_release = Some(0)`: unversioned files only this session wrote), so
  re-saving a brand-new instrument amends it until a project save.
- Saving the project (`save-project`) marks the session
  (`project_saved_since_release`), whether or not the write then succeeds.
- A cut renames the edited track's engine to the pinned id, so the next project
  save pins it. Tracks *sharing that engine* move with it: they already play the
  edited source live.

Other tracks in the open project that use an older release of the same
lineage, on their own engine, stay pinned. The release dropdown shows them, and a future "move all
tracks in this project to vN" action is out of scope here.

### 5.3 Pruning

None in this rev. Releases are small text files. A later "Delete unused
releases" in the dropdown would need the global reference scan this spec
avoids.

## 6. Inspector

Built (rev 1): its own tile, `*patch-inspector*`, right of `*fx*` (0.65 /
0.35, min 30 cells; the lower toggle shows or hides both). The patcher's
render pass mirrors the selection (`selected_nodes_for_path`) onto
`editor.selected-node-ids` / `selected-node-texts`; natives
`patcher-node-parse`, `patcher-node-set-attr`,
`patcher-operator-attributes`, `patcher-operator-doc` and
`patcher-set-node-texts` (one undo step, queued `:on-change`) do the rest.
It shows the operator's manifest docs (summary, then one entry per inlet and
outlet: name, kind, optional, what it does) above the attribute fields. Box
text is edited on the canvas only, and a box mid-edit is not inspected until
its edit commits. A draft's name is asked in a modal on its first Save (and
after Save as…), not by an always-on toolbar field. Saving an already-saved
patch has nothing to ask, so it saves at once and a toast says what it did:
`Saved pad v4 · projects using v3 keep it` (a new release), `Updated pad v4`
(amending this session's release), or `Saved pad` (unversioned / in place). Fields are text inputs committing on Enter;
drag numbers, role / mod-mode menus and instrument settings with nothing
selected are not built yet (the toolbar holds run mode; voices are the
instrument panel's).

`*patch-inspector*` fills the remaining bottom-bar width. It edits the
**selected patcher object**:

- **`param` boxes:** `name`, `@default`, `@min`, `@max`, `@unit`, `@group`,
  `@env`, `@role`, `@mod`, `@mod-mode` as typed fields. Number fields are drag
  numbers; `@role` and `@mod-mode` are menus over their known values.
- **`in` and `out` boxes:** index and `@name`.
- **Other objects:** the box text plus one field per `@attr` present.
- **Nothing selected:** instrument-level settings, namely run mode, voice
  controls (`voice_controls` in `instrument.json`) and base note.
- **Multiple selection:** fields shared by all selected boxes, with a mixed
  value shown as `—`. A write applies to all of them in one undo step.

Edits rewrite the box's text tokens through the existing patcher write-back
path and go through patcher undo (the history hook in
`set_patcher_interaction_state`). The inspector never holds state the box text
does not.

Payoff: param boxes no longer need to be read as 60-character attribute
strings. A later rev can collapse `param` boxes to `param attack` with
attributes shown only in the inspector. The same panel is the seed of the
future instrument UI builder, which would select a widget in the instrument
panel and edit its layout attributes here.

## 7. Build order

1. **User-tier versioning** (§5). Pure storage plus resolution, testable
   without UI: save twice, confirm two releases, and confirm a project pinned
   to v1 still loads v1.
2. **Toolbar + save flow** (§3, §4). The new toolbar buffer and the
   `editor-save`/`editor-save-as`/`editor-done` host commands; `editor-header`
   is removed from the patcher layout. Depends on 1 for the release dropdown
   and Save semantics. Run mode and breadcrumb can land first if 1 slips.
3. **Theme slots** (§3.1). Small; lands with 2 or right after.
4. **Panel toggles re-targeted** (§2.1). Independent of 2.
5. **Bottom-bar reflow** (§2): drop `*samples*` and Track FX, and lay out the
   mixer, instrument panel, and inspector placeholder.
6. **Inspector** (§6), starting with the `param` boxes.

## 8. Open questions

- Is changing `run_mode` live safe mid-session (voice allocator swap on a bound
  track)? If not, the toggle applies on next Save with a "takes effect on save"
  hint.
- Does the code surface want the toolbar's Save to imply Eval first? The
  proposal is yes: Save compiles, and a compile error blocks the save with the
  error shown in the toolbar.
- Should forks record `forked_from` (open in `instrument-fork-spec.md` §6)?
  The release dropdown is a natural place to show it.
