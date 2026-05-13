# `metal_seq` Modularization Spec

## Goal

Modularize `src/bin/metal_seq.rs` without behavior changes by extracting four low-risk modules first:

- `piano_roll.rs`
- `values.rs`
- `browser.rs`
- `custom_ui.rs`

This first increment should shrink the binary file while leaving the event loop, native registration, and host-command routing structurally intact.

## Target Layout

```text
src/bin/metal_seq/
  main.rs
  values.rs
  piano_roll.rs
  browser.rs
  custom_ui.rs
```

`main.rs` is the current `metal_seq.rs` moved into a binary directory, with module declarations:

```rust
mod browser;
mod custom_ui;
mod piano_roll;
mod values;
```

Cargo supports `src/bin/metal_seq/main.rs` as the same `metal_seq` binary.

## Phase 0: Mechanical Move

Move:

```text
src/bin/metal_seq.rs
```

to:

```text
src/bin/metal_seq/main.rs
```

No logic changes.

Acceptance:

```sh
cargo fmt
cargo check --bin metal_seq
```

Run broader tests if the move reveals unexpected coupling:

```sh
cargo test
```

## Phase 1: `values.rs`

Purpose: shared Lisp `Value` construction helpers.

Move these functions:

```rust
value_cell
map_value
list_value
build_string_list
build_flat_tree_items
```

Possibly also move generic parsing helpers if they are not piano-roll-specific:

```rust
value_as_number
value_as_usize
value_as_u64
value_as_keyword_or_string
cloned_map
```

Recommended API:

```rust
pub(crate) fn value_cell(value: Value) -> Rc<RefCell<Value>>;
pub(crate) fn map_value(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value;
pub(crate) fn list_value(values: impl IntoIterator<Item = Value>) -> Value;
pub(crate) fn build_string_list(items: &[String]) -> Value;
```

If `map_value` needs dynamic keys later, add a second helper instead of weakening this one immediately.

Acceptance:

```sh
cargo fmt
cargo check --bin metal_seq
```

## Phase 2: `piano_roll.rs`

Purpose: isolate piano-roll domain state, conversion, `Value` building, and action mutation logic.

Move constants:

```rust
PIANO_ROLL_ID_STRIDE
PIANO_ROLL_MIN_TRANSPOSE
PIANO_ROLL_MAX_TRANSPOSE
PIANO_ROLL_MIN_DURATION
```

Move structs:

```rust
PianoRollNote
PianoRollMoveItem
PianoRollMoveState
```

Move functions:

```rust
piano_roll_sanitize_duration
piano_roll_lane_to_transpose
piano_roll_transpose_to_lane
piano_roll_transpose_label
piano_roll_item_id
piano_roll_item_parts
build_piano_roll_lanes_value
piano_roll_step_note_entries
set_piano_roll_step_note_entries
piano_roll_find_note_index
build_piano_roll_items_value
build_piano_roll_selection_value
sync_piano_roll_state
parse_piano_roll_ids
piano_roll_action_mutates_pattern
apply_piano_roll_action
move_piano_roll_items_by_delta
move_piano_roll_items_absolute
```

Use `crate::values::{list_value, map_value}` from inside the module.

Recommended visibility:

```rust
pub(crate) struct PianoRollMoveState { ... }

pub(crate) fn build_piano_roll_lanes_value() -> Value;
pub(crate) fn build_piano_roll_items_value(...) -> Value;
pub(crate) fn build_piano_roll_selection_value(...) -> Value;
pub(crate) fn sync_piano_roll_state(...) -> ();
pub(crate) fn piano_roll_action_mutates_pattern(action: &Value) -> bool;
pub(crate) fn apply_piano_roll_action(...) -> Result<String, String>;
```

Keep helper functions private unless `main.rs` still needs them.

Acceptance:

```sh
cargo fmt
cargo check --bin metal_seq
cargo test
```

## Phase 3: `browser.rs`

Purpose: isolate sample, instrument, project, and preset tree building and filtering.

Move structs:

```rust
SampleTreeNode
InstrumentTreeNode
```

Move functions:

```rust
build_sample_tree_node
sample_tree_nodes_to_value
filter_sample_tree_nodes
build_instrument_tree_nodes
instrument_tree_nodes_to_value
filter_instrument_tree_nodes
build_instrument_tree_value
build_flat_tree_items
visible_project_items
build_project_tree
build_preset_tree_from_list
visible_preset_items_for_track
instrument_display_name
```

`visible_preset_items_for_track` depends on `ui::App`, so either keep it here as a browser/sidebar helper or defer it until sidebar extraction. If extracting it causes messy imports, leave it in `main.rs` temporarily.

Recommended API:

```rust
pub(crate) fn build_sample_tree_node(dir: &Path) -> Vec<SampleTreeNode>;
pub(crate) fn sample_tree_nodes_to_value(items: &[SampleTreeNode]) -> Value;
pub(crate) fn filter_sample_tree_nodes(
    items: &[SampleTreeNode],
    query_lower: &str,
) -> Vec<SampleTreeNode>;
pub(crate) fn build_instrument_tree_value(query: &str) -> Value;
pub(crate) fn build_project_tree(query: &str) -> Value;
pub(crate) fn build_preset_tree_from_list(items_value: Option<&Value>, query: &str) -> Value;
```

Acceptance:

```sh
cargo fmt
cargo check --bin metal_seq
```

## Phase 4: `custom_ui.rs`

Purpose: isolate custom instrument UI source generation and reload logic.

Move functions:

```rust
lisp_string_literal
expr_to_lisp
custom_ui_param_name
transform_synth_ui_expr
safe_lisp_ident
build_custom_instrument_ui_source_with_overlay
reload_custom_instrument_ui
active_custom_ui_buffer_overlay
```

Recommended API:

```rust
pub(crate) fn lisp_string_literal(value: &str) -> String;
pub(crate) fn reload_custom_instrument_ui(editor: &mut Editor);
pub(crate) fn build_custom_instrument_ui_source_with_overlay(
    overlay: Option<(String, String, String)>,
) -> String;
```

Keep parser transformation helpers private.

Acceptance:

```sh
cargo fmt
cargo check --bin metal_seq
cargo test
```

## Rules For This Increment

- No behavior changes.
- No renames unless required by visibility or imports.
- No restructuring of the event loop yet.
- No restructuring of native registration yet.
- No restructuring of host command routing yet.
- Use `pub(crate)` only where `main.rs` needs access.
- Keep everything else private to the new module.

## Final Acceptance

After all four module extractions:

```sh
cargo fmt
cargo check --bin metal_seq
cargo test
```

Expected result: `main.rs` should lose roughly 1,500 to 2,500 lines while still containing the large event loop and runtime registration. That is acceptable; those are the next extraction pass.
