# Scene Banks Spec

Status: **BUILT** — implemented and acceptance-swept 2026-08-27 (epic `eseq-doy`).
Rev 2 (2026-08-27, `eseq-doy.9`) adds §10: the mixer clip grid is bank-scoped
too, which rev 1 had excluded.

## 1. Goal

Divide the scene list into switchable **banks** so large sets (the
hundreds-of-scenes live rig in `docs/rack-scene-swap-spec.md` §2) stay
navigable. The transport scene strip shows one bank at a time; a dropdown after
the `+`/`-` buttons switches the viewed bank. Banking is a
**presentation/organization layer only**: scene identity, launch semantics,
and arrangement references are unchanged, and switching the viewed bank does
zero engine work.

Naming note: "bank" is overloaded in this codebase (wavetable banks, preset
banks, and — dangerously — the legacy "pattern bank" name for the scene list
itself in `scenes-and-track-patterns-spec.md` and `project.rs`). Code for this
feature always says **scene bank** (`SceneBank`, `scene_banks`,
`create-scene-bank`); the UI just says "Bank".

## 2. Bank model (design question 1)

**Banks are ordered, variable-size, contiguous spans over the existing
`ProjectScenes.scenes: Vec<Scene>`.** The flat scene Vec keeps its role as the
single presentation order; banks are boundaries over it, never a second
ordering. Consequences:

- Every scene belongs to exactly one bank (a partition, by construction).
- Bank k's scenes are `scenes[offset(k) .. offset(k) + len(k)]`.
- **Capacity: max 24 scenes per bank** — 24 is what the scene-strip container
  fits. `+` is disabled (with a hint) when the viewed bank is full.
- Empty banks are allowed (create a bank, then add scenes to it).
- At least one bank always exists. Sum of bank lengths == scene count at all
  times (checked alongside the id-space invariant in
  `sequencer_state/accessors.rs:1694`).
- Banks are auto-named by position: **A, B, C, … Z, AA, AB, …** A bank may
  optionally carry a user name (rename op exists, dropdown shows
  `"B — Peak"` style when named), but auto letters are the default and letters
  always reflect current position (they are labels, not identity).

```rust
// in sequencer/state/scenes.rs, alongside ProjectScenes
pub struct SceneBank {
    pub id: SceneBankId,        // stable u64, minted like SceneId
    pub name: Option<String>,   // None => auto letter from position
    pub len: usize,             // number of scenes in this bank's span
}
// ProjectScenes gains: banks: Vec<SceneBank>, next_bank_id: u64
```

Offsets are derived (prefix sums), not stored. `SceneBankId` is stable across
reorders so UI selection and undo labels can address a bank; scene-facing code
never needs it.

Rejected alternatives:
- *Membership side-lists (track-groups style)*: allows non-contiguous banks,
  but scenes already have one global order that banks naturally partition, and
  side-lists of indices need remapping on every scene create/delete/reorder.
- *Fixed 16-slot Elektron banks / pure pagination*: rejected in design review;
  variable size with an explicit cap won.
- *Membership by `SceneId`*: `SceneId` is not persisted today
  (`scenes.rs:327` regenerates ids from index on load); making it durable is a
  real serialization semantics change (version bump) that banking does not
  need. The contiguous model sidesteps it entirely.

## 3. Data model + serialization (design question 2)

Follow the track-groups precedent (`ProjectFile.groups`, `project.rs:135`):
a **serde-default side field, no `PROJECT_FILE_VERSION` bump** — old readers
ignore it, and a missing/empty field loads as "all scenes in one bank A".

```rust
// project.rs
#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectSceneBank {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub len: usize,
}
// ProjectFile / ProjectFileWire:
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub scene_banks: Vec<ProjectSceneBank>,
```

Load-time validation (in the `from_pattern_snapshots` path / project load):
if `scene_banks` is empty, missing, or inconsistent (lengths don't sum to the
scene count, a len > 24, zero banks), fall back to consecutive unnamed banks
of at most 24 scenes rather than failing the load. Thus every ordinary legacy
project with at most 24 scenes loads wholly into bank A; larger legacy projects
load into A/B/… chunks so the capacity invariant remains true. `next_bank_id`
is re-derived as `max(id) + 1` on load (ids need only be unique within a
project; like `SceneId` they may be re-minted if absent).

Unlike scene names (a known persistence gap — not fixed here), **bank names
and boundaries are persisted**.

## 4. Interaction (design question 3)

Surface: the transport pattern-pill strip (`content/ui/transport.lisp:885`,
subtree `"transport-pattern-pills"`).

- The strip's `each` enumerates **only the viewed bank's scenes**: local pill
  `i` maps to global scene `bank_offset + i`. Pills are numbered locally
  (1‥24) — the bank letter in the dropdown provides context, Elektron-style.
- **A dropdown sits after the `+` and `-` buttons** showing the current bank
  letter (styled like the existing preset dropdown, e.g. the sampler-panel
  `main ▾` selector). Opening it lists all banks plus a "New bank" entry;
  selecting an entry switches the viewed bank. Right-clicking the selector
  opens operations for the viewed bank: **Rename bank** enters an inline,
  submit/cancel rename field, and **Delete bank** merges it according to §7.
  Delete is disabled when only one bank remains or the merge target would
  exceed 24 scenes.
- **Viewed bank is pure UI state** (Lisp `defstate` in transport.lisp), not
  engine state and not persisted; on load it initializes to the bank containing
  the current scene. Switching it re-renders the strip and nothing else — no
  host command, no scheduler traffic, no snapshot capture.
- **Viewing a different bank never touches playback.** The playing scene keeps
  playing; if it lives outside the viewed bank, the bank dropdown shows a
  subtle **indicator** (dot/pulse in the playing bank's hue) marking which
  bank holds the playing scene. No auto-follow in v1.
- If a structural edit removes the viewed bank (delete bank, undo), the view
  falls back to the previous bank (index-clamped).
- **`-` is disabled when the current scene is not in the viewed bank** (it
  still deletes the current scene when enabled — no risk of deleting a scene
  you can't see).

Rust → Lisp feed: a new reactive value `SEQ.scene-banks` — list of
`(dict :id :label :name :len :offset)` — published from
`ui/state_values/song_state.rs` next to `SEQ.scene-names` (`:858`), with the
same epoch-cached pattern. `SEQ.current-pattern` (global index) plus this
table is enough for the Lisp side to derive the viewed-bank slice, local
numbering, and the playing-bank indicator; no new per-frame work.

## 5. Launch semantics (design question 4)

**Unchanged, by construction.** Scene launch and quantized launch address
scenes by global index (`PatternLaunchTarget::Scene { scene: usize }`,
`quantized_launch.rs:62`; `App::apply_pattern_launch_at`, `app/mod.rs:1596`)
and banking never renumbers a scene except through the ordinary
`reorder_scene` path it already goes through today. Clicking a pill in any
bank fires the same `switch-pattern`/launch host commands with the mapped
global index. Bank switching submits nothing to the mailbox and invalidates
nothing in the scheduler — see `QUANTIZED_SCENE_LAUNCH_FOUNDATION_SPEC.md` §9;
that seam is untouched.

## 6. Arrangement / song mode (design question 5)

Arrangement scene rows reference scenes by index
(`SceneEvent { scene: usize }`, `arrangement.rs:29`). Bank operations that do
not move scenes (create/rename/delete-empty bank, switching the viewed bank)
cannot affect them.

Moving a scene between banks reorders the scene Vec, and **`reorder_scene`
today does not remap arrangement `scene_lane` events**
(`scene_launch.rs:1317`) — a pre-existing bug (scene delete *does* remap macro
targets via `remap_scene_targets_after_delete`, `app/mod.rs:1901`; reorder
remaps nothing). **Fixing reorder remapping (arrangement scene events + macro
scene targets) is a prerequisite bead of this epic**, so that "Move to bank"
is safe. With that fix, arrangement references survive every bank op.

## 7. Editing ops + undo (design question 6)

All ops are host commands (module `ui/host_commands/scene_banks.rs`, following
`host_commands/scenes.rs`) and all are undoable **for free** via the existing
whole-object memento: banks live inside `ProjectScenes`, so
`apply_recorded_scene_structure_mutation` / `EditPatch::SceneStructure`
(`app/edit.rs:2151`, `history.rs:185`) already capture and restore them.

| Op | Host command | Semantics |
|---|---|---|
| Create bank | `create-scene-bank` | Appends an empty bank after the last; UI switches the view to it. |
| Rename bank | `rename-scene-bank` | Sets/clears the optional name; letters stay positional. |
| Delete bank | `delete-scene-bank` | Its scenes merge into the **previous** bank (first bank merges forward); refuses if the merge target would exceed 24, and refuses to delete the last remaining bank. Scene indices shift only if scenes actually move — deleting bank k merges span k into k-1, which is index-neutral (spans are adjacent), so arrangement refs are untouched. |
| Move scene to bank | `move-scene-to-scene-bank` | Reorders the scene to the end of the target bank's span + adjusts two `len`s; refuses if target is full. Exposed as a right-click context menu on a scene pill ("Move to bank …"). Runs through the fixed reorder remapping (§6). |
| Add scene (scoped `+`) | existing `clone-pattern`, gains an insert position | New scene is inserted **at the end of the viewed bank** and that bank's `len` grows. Needs an insert-at-index path in `new_scene` (`scenes.rs:1046` can only append today) with the same downstream remapping as reorder. Disabled at 24. |
| Delete scene (scoped `-`) | existing `delete-pattern` | Deletes the current scene as today; the owning bank's `len` shrinks, and an emptied bank simply stays empty. The button is disabled when the current scene is outside the viewed bank. |

The initial UI shipped bank switching + "New bank" + "Move to bank". The
follow-up UI exposes `rename-scene-bank` and `delete-scene-bank` from a context
menu on the current bank selector. Both commands continue through the same
scene-structure history transaction, so rename, rename-clear, and delete are
undoable.

Existing scene reorder (drag within the strip) stays within-bank: drag
source/target are both local indices mapped to global before the existing
`reorder-scene` command; cross-bank movement is only via "Move to bank".

## 8. Perf notes

Scene-switch perf work (subtree-scene-switch, scene/clip launch) assumed a
flat scene list; nothing here changes that — the flat Vec is still the model.
Bank switching is a Lisp-state change re-rendering ~24 pills. The
`SEQ.scene-banks` value changes only on structural edits (same cadence as
`SEQ.scene-names`), so drags and playback publish nothing new.

## 9. Built status and deviations

The implementation follows the agreed rev 1 model and interaction contract,
plus the completed rename/delete UI follow-up. The acceptance sweep covers
global-index quantized launch into another bank, index-neutral non-empty bank
deletion, arrangement and scene-macro reference identity, mixed bank/scene undo
and redo, legacy load defaults, serialization of names and boundaries, the
bank-filtered transport UI, and inline rename/delete command routing from the
bank selector context menu.

One contradiction in rev 1 was resolved during implementation: the original
serialization section said every missing/invalid bank table became one bank A,
while the model requires every bank to contain at most 24 scenes. As clarified
in §3, projects with more than 24 scenes are chunked into unnamed A/B/… banks;
projects of 24 scenes or fewer still load entirely into A. There are no other
known deviations from the agreed v1 scope. Keyboard bank switching remains
intentionally deferred rather than partially implemented.

## 10. Mixer clip grid (rev 2)

The per-track clip grid in the mixer (`track-pattern-grid`,
`content/ui/mixer.lisp`) is bank-scoped too, superseding rev 1's exclusion of
it. Without this the grid renders every pattern in a track's pool into a fixed
6x4 container and clips beyond ~24 overflow it — the same problem banking
solves for the scene strip.

### 10.1 Membership

A clip belongs to the banks whose scenes reference it: bank k contains clip
`p` on track `t` if some scene in bank k's span has `cells[t] == p`. Clips are
pool patterns, not scenes, so this is derived, not stored — no engine or
serialization change. Consequences:

- A bank holds at most 24 scenes, so at most 24 referenced clips per track —
  exactly the grid's capacity.
- A clip reachable from two banks renders in both. That is the honest view: it
  is one clip, launchable from either bank.
- **Orphans** — clips no scene in any bank references (a freshly cloned clip,
  a clip whose only scene was deleted) — render in *every* bank. Hiding them
  would strand a new clip behind a bank the user cannot guess, and the first
  click on one assigns it to the current scene, which gives it a bank.

Host feed: `SEQ.track-pattern-cells` cells gain a `:banks` field — the list of
bank indices referencing that clip, empty for orphans — built in
`build_track_pattern_cells_value` from
`ProjectScenes::track_pattern_bank_indices`. The viewed bank stays pure Lisp
state, so the host publishes membership rather than a pre-sliced list.

### 10.2 Which bank view

The grid follows the **transport's viewed bank** — one bank view for the whole
session, so switching banks in the strip re-scopes every track's clips at
once. That shared state moved out of `ui/transport.lisp` into a new
`eseq.scene-banks` module (`content/ui/scene-banks.lisp`) that both render
roots import: `ui/main.lisp` loads `ui/mixer.lisp` *before* `ui/transport.lisp`,
so a transport-owned `defstate` would not exist when the mixer's readers
compile. The module is a state/accessor hub with no `effect-buffer`, so
importing it from a render root is safe. Bank switching remains a pure view
change: no host command, no engine work.

## 11. Out of scope

- Persisting scene names / durable `SceneId` (known gap, unchanged).
- Auto-follow of the viewed bank on launch (indicator only in v1).
- Keyboard/pad access for bank switching (mouse-only dropdown).
- Cross-bank drag of pills, bank reordering UI, per-bank colors.
- Cross-bank drag of clips in the mixer grid; a per-clip durable bank
  assignment independent of the scenes that reference it.
