//! The Filter Table response editor session (spec §14.2p, eseq-0l17.56):
//! the `table-editor` singleton.
//!
//! The session is one global (`filter_table_editor`), outside the `App`,
//! bound to one effect node. Every field is live: while one is observed the
//! tick compares the session's revision (moved by every access that can
//! change it) and the device sources' generation with the last read, and
//! only when either moved (or the observed set did) reads the session once
//! and pushes the observed fields, each compared with its cell. `device` is
//! the device instance whose effect node the session edits, resolved over
//! the registered devices; a closed session, or one whose node no device
//! names, reads nil. What a device's node is can move with no revision (a
//! bus effect's node is read from the bus mirror, which can lag a rebuild
//! the session was reattached across), so while a session is open the tick
//! also checks that the resolved device still names the session's node (one
//! node read), and while none does, looks again (a walk over the Filter
//! Table devices) until one does. The band is on the response-curve-editor's axes
//! (`ParametricNode::curve_band`, shared with the legacy panel's `:editor`
//! map).

use super::*;
use sequencer::effects::filter_table;
use sequencer::effects::filter_table_editor::{self as fte, SessionUiState};

/// What the tick last read the session under.
#[derive(Default)]
pub(crate) struct TableEditorState {
    /// (session revision, device sources' generation, observed mask) at the
    /// last read; `None` forces one.
    seen: Option<(u64, u64, u32)>,
    /// The session's node and the device instance that read resolved.
    node: Option<i32>,
    device: Option<InstanceId>,
    /// Session reads, for tests.
    pub(crate) reads: u64,
}

impl TableEditorState {
    /// Read the session again at the next tick (a schema change).
    pub(super) fn invalidate(&mut self) {
        self.seen = None;
    }
}

/// Whether live device `id` (`source`) is a Filter Table whose effect node
/// is `node_id`.
fn names_node<S: KindStore>(
    store: &S,
    sources: &KindsHandles,
    (id, source): (InstanceId, &DeviceSource),
    node_id: i32,
) -> bool {
    store.kind_of(id) == Some(DEVICE)
        && source.desc.desc.name == filter_table::NAME
        && effect_node(sources, source).is_some_and(|node| node as i32 == node_id)
}

/// The registered device whose effect node is `node_id`: a Filter Table on
/// a track's or a bus's chain (or a rack slot's).
fn editor_device<S: KindStore>(
    store: &S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    node_id: i32,
) -> Option<InstanceId> {
    let shared = shared.borrow();
    let mut devices = shared.devices.iter();
    let (id, _) =
        devices.find(|(id, source)| names_node(store, sources, (**id, source), node_id))?;
    Some(*id)
}

/// Whether the device the last read resolved for an open session's node no
/// longer names it, or none did and one does now.
fn binding_moved<S: KindStore>(
    store: &S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    state: &TableEditorState,
) -> bool {
    let Some(node) = state.node else {
        return false;
    };
    match state.device {
        Some(device) => {
            let source = shared.borrow().devices.get(&device).cloned();
            !source.is_some_and(|source| names_node(store, sources, (device, &source), node))
        }
        None => editor_device(store, sources, shared, node).is_some(),
    }
}

/// Field `key` of the session `ui` (nil, false, 0 and "" while closed),
/// bound to `device`; `None` for another key.
fn table_editor_value(
    ui: Option<&SessionUiState>,
    device: Option<InstanceId>,
    key: FieldKey,
) -> Option<Value> {
    let band = ui.and_then(|ui| ui.band).map(|node| node.curve_band());
    let band_number = |read: fn(&fte::CurveBand) -> f64| number(band.as_ref().map_or(0.0, read));
    Some(match key {
        f::TABLE_EDITOR_DEVICE => instance_or_nil(device),
        f::TABLE_EDITOR_OPEN => Value::Bool(ui.is_some()),
        f::TABLE_EDITOR_FRAMES => number(ui.map_or(0, |ui| ui.frames) as f64),
        f::TABLE_EDITOR_SELECTED_FRAME => number(ui.map_or(0, |ui| ui.selected_frame) as f64),
        f::TABLE_EDITOR_SELECTED_FRAME_NORMALIZED => {
            number(ui.map_or(0.0, SessionUiState::selected_frame_normalized))
        }
        f::TABLE_EDITOR_CAN_UNDO => Value::Bool(ui.is_some_and(|ui| ui.can_undo)),
        f::TABLE_EDITOR_CAN_REDO => Value::Bool(ui.is_some_and(|ui| ui.can_redo)),
        f::TABLE_EDITOR_DIRTY => Value::Bool(ui.is_some_and(|ui| ui.dirty)),
        f::TABLE_EDITOR_OP_COUNT => number(ui.map_or(0, |ui| ui.op_count) as f64),
        f::TABLE_EDITOR_BAND_KIND => text(band.map_or("", |band| band.kind.tag())),
        f::TABLE_EDITOR_BAND_FREQ => band_number(|band| band.freq),
        f::TABLE_EDITOR_BAND_GAIN => band_number(|band| band.gain),
        f::TABLE_EDITOR_BAND_Q => band_number(|band| band.q),
        _ => return None,
    })
}

/// One `table-editor` field, read now (the reader hook's cold read).
pub(super) fn table_editor_live_value<S: KindStore>(
    store: &S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    key: FieldKey,
) -> Option<Value> {
    let ui = fte::session_ui_state();
    // Only `device` resolves the node (a walk over the device sources).
    let device = (ui.as_ref())
        .filter(|_| key == f::TABLE_EDITOR_DEVICE)
        .and_then(|ui| editor_device(store, sources, shared, ui.node_id));
    table_editor_value(ui.as_ref(), device, key)
}

impl HostKinds {
    /// The `table-editor` singleton's observed fields, when the session's
    /// revision, the device sources or the observed set moved since the
    /// last read, or the device binding did (see the module docs).
    pub(super) fn sync_table_editor(&mut self, pusher: &mut Pusher<'_>) {
        let Some(id) = pusher.singleton(TABLE_EDITOR) else {
            return;
        };
        let state = &mut self.table_editor;
        let mask = pusher.rt.host_fields_observed(id, &TABLE_EDITOR_LIVE.names);
        if mask == 0 {
            state.seen = None;
            return;
        }
        let key = (
            fte::session_revision(),
            pusher.shared.borrow().devices_generation,
            mask,
        );
        let (sources, shared) = (pusher.sources, pusher.shared);
        if state.seen == Some(key) && !binding_moved(&*pusher.rt, sources, shared, state) {
            return;
        }
        state.seen = Some(key);
        state.reads += 1;
        let ui = fte::session_ui_state();
        state.node = ui.as_ref().map(|ui| ui.node_id);
        state.device =
            (state.node).and_then(|node| editor_device(&*pusher.rt, sources, shared, node));
        for (bit, field) in TABLE_EDITOR_LIVE.keys.iter().enumerate() {
            if mask & (1 << bit) == 0 {
                continue;
            }
            if let Some(value) = table_editor_value(ui.as_ref(), state.device, *field) {
                pusher.push_computed(id, *field, value);
            }
        }
    }
}
