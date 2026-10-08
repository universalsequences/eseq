//! Stage 7b-5: the Filter Table response editor session as the
//! `table-editor` singleton (spec §14.2p), against a real Filter Table.

use super::*;
use sequencer::effects::filter_table;
use sequencer::effects::filter_table_editor as fte;

const REFER_TE: &str = "(import eseq.kinds :refer (track buses table-editor \
                        table-editor-open! table-editor-close! table-editor-band! \
                        table-editor-op! table-editor-add-node! table-editor-frame! \
                        table-editor-undo! table-editor-redo! table-editor-save!))";

/// The fields, in the order [`session_fields`] lists the session map's.
const FIELDS: &str = "(list te.open te.frames te.selected-frame te.selected-frame-normalized \
                      te.can-undo te.can-redo te.dirty te.op-count \
                      te.band-kind te.band-freq te.band-gain te.band-q)";

/// The `table-editor` field names, as `FIELDS` reads them.
const NAMES: [&str; 12] = [
    "open",
    "frames",
    "selected-frame",
    "selected-frame-normalized",
    "can-undo",
    "can-redo",
    "dirty",
    "op-count",
    "band-kind",
    "band-freq",
    "band-gain",
    "band-q",
];

impl Harness {
    fn eval_te(&mut self, code: &str) -> Value {
        let source = format!("{REFER_TE}\n(def te table-editor)\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    /// Run `code`'s commands as the event loop does, then one tick.
    fn act(&mut self, code: &str) {
        self.editor.minibuffer = None;
        self.eval_te(code);
        self.drain();
        self.sync();
    }

    /// A Filter Table on track 0, its device bound to `ft`; returns its
    /// slot and node.
    fn track_filter_table(&mut self) -> (usize, i32) {
        let slot = self.add_effect(0, filter_table::NAME);
        self.sync();
        let node = self.shared.state.pattern.effect_chains[0][slot]
            .node_id
            .load(Ordering::Relaxed) as i32;
        assert!(
            filter_table::prepared_table_for(node).is_some(),
            "a default table"
        );
        let position = self.chain_position(0, slot);
        self.eval_te(&format!(
            "(def ft (nth (let ((t (track 0))) t.devices) {position}))"
        ));
        assert_eq!(self.eval_te("ft.name"), s(filter_table::NAME));
        (slot, node)
    }

    /// The position of chain slot `slot` among track `track`'s devices.
    fn chain_position(&mut self, track: usize, slot: usize) -> usize {
        let names = items(&self.eval_te(&format!(
            "(map (lambda (d) d.slot) (let ((t (track {track}))) t.devices))"
        )));
        (names.iter())
            .position(|listed| *listed == Value::Number(slot as f64))
            .expect("the slot's device")
    }

    fn te_reads(&self) -> u64 {
        self.frame.host_kinds.table_editor.reads
    }

    /// The singleton's cells, in `FIELDS` order.
    fn te_cells(&self) -> Vec<Value> {
        let id = self.singleton(TABLE_EDITOR);
        (NAMES.iter())
            .map(|name| self.rt().instance_field(id, name).expect(name))
            .collect()
    }
}

/// The session map ([`session_editor`]) as the kind's fields: the band's entries flat,
/// the closed defaults where the map (or its band) is absent.
fn session_fields(editor: &Value) -> Vec<Value> {
    if *editor == Value::Nil {
        let (no, zero) = (Value::Bool(false), Value::Number(0.0));
        let mut closed = vec![no.clone(), zero.clone(), zero.clone(), zero.clone()];
        closed.extend([no.clone(), no.clone(), no, zero.clone(), s("")]);
        closed.extend([zero.clone(), zero.clone(), zero]);
        return closed;
    }
    let band = get(editor, "band");
    let band_field = |key: &str| match &band {
        Value::Nil => Value::Number(0.0),
        band => get(band, key),
    };
    let mut fields: Vec<Value> = NAMES[..8].iter().map(|key| get(editor, key)).collect();
    fields.push(match &band {
        Value::Nil => s(""),
        band => get(band, "kind"),
    });
    fields.extend(["freq", "gain", "q"].map(band_field));
    fields
}

/// The editor session as a map (the band nested), when it is bound to
/// effect node `node_id`; nil otherwise: what the kind's fields must read.
fn session_editor(node_id: u32) -> Value {
    let Some(ui) = fte::session_ui_state().filter(|ui| ui.node_id as u32 == node_id) else {
        return Value::Nil;
    };
    let band = ui.band.map(|node| {
        let curve = node.curve_band();
        map_value([
            ("kind", s(curve.kind.tag())),
            ("freq", Value::Number(curve.freq)),
            ("gain", Value::Number(curve.gain)),
            ("q", Value::Number(curve.q)),
        ])
    });
    let mut entries = vec![
        ("open", Value::Bool(true)),
        ("frames", Value::Number(ui.frames as f64)),
        ("selected-frame", Value::Number(ui.selected_frame as f64)),
        (
            "selected-frame-normalized",
            Value::Number(ui.selected_frame_normalized()),
        ),
        ("can-undo", Value::Bool(ui.can_undo)),
        ("can-redo", Value::Bool(ui.can_redo)),
        ("dirty", Value::Bool(ui.dirty)),
        ("op-count", Value::Number(ui.op_count as f64)),
    ];
    entries.extend(band.map(|band| ("band", band)));
    map_value(entries)
}

/// The editor session as track 0's effect in `slot` sees it.
fn track_editor(h: &Harness, slot: usize) -> Value {
    let chain = &h.shared.state.pattern.effect_chains[0];
    session_editor(chain[slot].node_id.load(Ordering::Relaxed))
}

/// The kind's fields read by value, and (observed) its cells, match the
/// session.
fn assert_parity(h: &mut Harness, session: &Value, context: &str) {
    let expected = session_fields(session);
    assert_eq!(items(&h.eval_te(FIELDS)), expected, "{context}: by value");
    assert_eq!(h.te_cells(), expected, "{context}: pushed");
}

/// Observe every field (a view printing them).
fn observe(h: &mut Harness) {
    h.eval_te(&format!(
        r#"(effect-buffer "*te*" (label (str te.device {FIELDS})))"#
    ));
    h.show_all();
    h.sync();
}

fn close_session() {
    fte::set_session(None);
}

/// The session is one global: tests that open it run one at a time (a
/// thread-parallel `cargo test`; nextest runs each in its own process).
fn te_lock() -> std::sync::MutexGuard<'static, ()> {
    static TE_LOCK: Mutex<()> = Mutex::new(());
    TE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn the_table_editor_reads_the_editor_session() {
    let _lock = te_lock();
    let mut h = Harness::new();
    close_session();
    let (slot, node) = h.track_filter_table();
    // Closed: the defaults, the device nil, no session map.
    assert_eq!(track_editor(&h, slot), Value::Nil);
    assert_eq!(items(&h.eval_te(FIELDS)), session_fields(&Value::Nil));
    assert_eq!(h.eval_te("te.device"), Value::Nil);
    observe(&mut h);
    assert_parity(&mut h, &Value::Nil, "closed");

    h.act("(table-editor-open! ft)");
    assert_eq!(fte::session_ui_state().map(|ui| ui.node_id), Some(node));
    assert_eq!(h.eval_te("(= te.device ft)"), Value::Bool(true));
    let session = track_editor(&h, slot);
    assert_ne!(session, Value::Nil);
    assert_parity(&mut h, &session, "open");
    assert_eq!(h.eval_te("te.band-kind"), s(""), "no band before a node");

    // A band, then the frame selection: every field follows.
    h.act(r#"(table-editor-add-node! "notch")"#);
    h.act("(table-editor-frame! 12)");
    let session = track_editor(&h, slot);
    assert_eq!(get(&get(&session, "band"), "kind"), s("notch"));
    assert_parity(&mut h, &session, "a band");
    assert_eq!(h.eval_te("te.selected-frame"), Value::Number(12.0));
    // A non-band op hides the band, as the session map drops it.
    h.act(r#"(table-editor-op! "tilt" :value -3)"#);
    let session = track_editor(&h, slot);
    assert_eq!(get(&session, "band"), Value::Nil);
    assert_parity(&mut h, &session, "a tilt");
    h.act("(table-editor-undo!)");
    let session = track_editor(&h, slot);
    assert_parity(&mut h, &session, "undone");
    assert_eq!(
        h.eval_te("(list te.can-undo te.can-redo)"),
        h.eval_te("(list true true)")
    );

    h.act("(table-editor-close!)");
    assert_parity(&mut h, &Value::Nil, "closed again");
    assert_eq!(h.eval_te("te.device"), Value::Nil);
    close_session();
}

#[test]
fn a_bus_filter_tables_session_names_its_bus_device() {
    let _lock = te_lock();
    let mut h = Harness::new();
    close_session();
    let id = h.add_bus("FX");
    let bus = h.app.buses.iter().position(|bus| bus.id == id).unwrap();
    let slot = h
        .app
        .apply_recorded_bus_effect_chain_mutation(bus, "Add bus effect", |app| {
            app.add_builtin_bus_effect_sync(bus, filter_table::NAME)
        })
        .expect("add bus effect");
    h.share_buses_and_groups();
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    h.eval_te(&format!(
        "(def bft (first (let ((b (nth (buses) {bus}))) b.devices)))"
    ));
    assert_eq!(h.eval_te("bft.name"), s(filter_table::NAME));
    observe(&mut h);
    h.act(r#"(table-editor-open! bft) (table-editor-add-node! "peak")"#);
    assert_eq!(h.eval_te("(= te.device bft)"), Value::Bool(true));
    let session = session_editor(h.app.buses[bus].effect_slots[slot].node_id);
    assert_eq!(get(&get(&session, "band"), "kind"), s("peak"));
    assert_parity(&mut h, &session, "bus");

    // An engine swap rebuilds the node and carries the session across;
    // the bus mirror the device's node is read from lags it (the event
    // loop refreshes it later), so the device reads nil until the mirror
    // names the new node, then the same device again, with no session edit.
    let old_node = fte::session_ui_state().expect("open").node_id;
    h.app
        .apply_recorded_bus_effect_chain_mutation(bus, "Set bus Filter Table engine", |app| {
            app.set_bus_filter_table_engine(bus, slot, filter_table::TableEngine::Causal)
        })
        .expect("engine swap");
    let new_node = fte::session_ui_state().expect("reattached").node_id;
    assert_ne!(new_node, old_node, "a new node");
    h.sync();
    assert_eq!(h.te_cells()[0], Value::Bool(true), "still open");
    let id = h.singleton(TABLE_EDITOR);
    assert_eq!(
        h.rt().instance_field(id, "device").expect("device"),
        Value::Nil
    );
    let reads = h.te_reads();
    h.sync();
    assert_eq!(h.te_reads(), reads, "no device names the node yet: no read");
    h.share_buses_and_groups();
    h.sync();
    assert_eq!(h.eval_te("(= te.device bft)"), Value::Bool(true));
    assert_eq!(h.te_reads(), reads + 1);
    h.act("(table-editor-close!)");
    close_session();
}

#[test]
fn the_table_editor_reads_the_session_only_while_observed_and_when_it_moves() {
    let _lock = te_lock();
    let mut h = Harness::new();
    close_session();
    let (_, _) = h.track_filter_table();
    // Unobserved: no read, whatever the session does.
    h.act("(table-editor-open! ft)");
    h.act(r#"(table-editor-add-node! "peak")"#);
    assert_eq!(h.te_reads(), 0);
    let computed = h.computed(f::TABLE_EDITOR_OP_COUNT);
    h.sync();
    assert_eq!(h.computed(f::TABLE_EDITOR_OP_COUNT), computed);
    // Observed: one read, then none while idle.
    observe(&mut h);
    let reads = h.te_reads();
    assert!(reads >= 1);
    let computed = h.computed(f::TABLE_EDITOR_OP_COUNT);
    for _ in 0..3 {
        assert!(!h.sync(), "an idle tick changes nothing");
    }
    assert_eq!(h.te_reads(), reads, "idle ticks read no session");
    assert_eq!(h.computed(f::TABLE_EDITOR_OP_COUNT), computed);
    // An edit moves the revision: one read, the changed fields pushed.
    h.act(r#"(table-editor-op! "normalize")"#);
    assert_eq!(h.te_reads(), reads + 1);
    assert_eq!(h.te_cells()[7], Value::Number(2.0), "op-count");
    // A frame selection moves it too.
    h.act(r#"(table-editor-frame! 3)"#);
    assert_eq!(h.te_reads(), reads + 2);
    assert!(!h.sync());
    // A band drag's preview auditions without an edit: no revision, no read.
    let revision = fte::session_revision();
    h.act(r#"(table-editor-band! "peak" 96 3 2)"#);
    assert_eq!(fte::session_revision(), revision);
    assert_eq!(h.te_reads(), reads + 2);
    h.act("(table-editor-close!)");
    close_session();
}

#[test]
fn the_table_editor_actions_apply_like_the_legacy_commands() {
    let _lock = te_lock();
    let mut h = Harness::new();
    close_session();
    let (_, node) = h.track_filter_table();
    let ui = || fte::session_ui_state().expect("an open session");
    // Nothing open: an action is an error that opens nothing.
    h.act("(table-editor-undo!)");
    assert!(
        h.error().contains("no Filter Table editor open"),
        "{}",
        h.error()
    );
    h.act("(table-editor-open! ft)");
    assert_eq!(ui().node_id, node);
    assert_eq!((ui().op_count, ui().dirty), (0, false));

    // A band drag: change auditions without an edit, commit adds one, a
    // second commit replaces it (the session command's coalescing).
    h.act(r#"(table-editor-band! "peak" 96 6 2)"#);
    assert_eq!(ui().op_count, 0, "a change is a preview");
    h.act(r#"(table-editor-band! "peak" 96 6 2 :phase "commit")"#);
    assert_eq!(ui().op_count, 1);
    let band = ui().band.expect("a band").curve_band();
    assert_eq!((band.freq, band.gain, band.q), (96.0, 6.0, 2.0));
    h.act(r#"(table-editor-band! "peak" 48 -3 4 :phase "commit")"#);
    assert_eq!(ui().op_count, 1, "the drag replaced its band");
    assert_eq!(
        h.eval_te("(list te.band-freq te.band-gain te.band-q)"),
        h.eval_te("(list 48 -3 4)")
    );

    // Ops and frame ops with their options; the frame setter.
    h.act(r#"(table-editor-op! "duplicate-frame")"#);
    assert_eq!(ui().frames, filter_table::FRAMES + 1);
    h.act("(set! te.selected-frame 5)");
    assert_eq!(ui().selected_frame, 5);
    h.act(r#"(table-editor-op! "smooth-spectral" :radius 2 :frame-start 0 :frame-end 4)"#);
    assert_eq!(ui().op_count, 3);
    h.act(&format!(
        "(set! te.selected-frame {})",
        filter_table::FRAMES + 1
    ));
    assert!(h.error().contains("out of range"), "{}", h.error());
    assert_eq!(
        ui().selected_frame,
        5,
        "an out-of-range frame changes nothing"
    );
    for frame in ["2.5", "-1"] {
        h.act(&format!("(table-editor-frame! {frame})"));
        assert!(
            h.error().contains("not a frame index"),
            "{frame}: {}",
            h.error()
        );
        assert_eq!(ui().selected_frame, 5, "{frame} is never truncated");
    }
    h.act("(set! te.selected-frame -1)");
    assert!(h.error().contains("not a frame index"), "{}", h.error());
    assert_eq!(ui().selected_frame, 5);
    h.act(r#"(table-editor-add-node! "wobble")"#);
    assert!(h.error().contains("unknown node kind"), "{}", h.error());
    assert_eq!(ui().op_count, 3);

    // The editor's own history.
    h.act("(table-editor-undo!) (table-editor-undo!)");
    assert_eq!((ui().can_undo, ui().can_redo), (true, true));
    assert_eq!(ui().frames, filter_table::FRAMES);
    h.act("(table-editor-redo!)");
    assert_eq!(ui().frames, filter_table::FRAMES + 1);

    // Save: an asset, loaded into the device through project history.
    let stem = "kinds-table-editor";
    let dir = std::env::temp_dir().join(format!("{stem}-{}", std::process::id()));
    h.app.filter_table_save_dir = Some(dir.clone());
    let undo_len = h.app.history.undo_len();
    h.act(&format!(r#"(table-editor-save! :name "{stem}")"#));
    let path = dir.join(format!(
        "{stem}.{}",
        sequencer::effects::filter_table_asset::EXTENSION
    ));
    assert!(path.exists(), "{}: {}", path.display(), h.error());
    assert_eq!(h.app.history.undo_len(), undo_len + 1, "one project entry");
    assert!(!ui().dirty);
    assert_eq!(h.eval_te("te.dirty"), Value::Bool(false));
    assert_eq!(
        filter_table::table_ref_for(node).as_deref(),
        Some(sequencer::effects::filter_table_asset::encode_asset_ref(stem).as_str())
    );

    // A device that is no track or bus effect has no editor.
    h.app
        .graph_controller()
        .add_blank_sampler_track()
        .expect("sampler track");
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    h.act("(table-editor-open! (first (let ((t (track 2))) t.devices)))");
    assert!(h.error().contains("response editor"), "{}", h.error());
    assert_eq!(ui().node_id, node, "the open session stays");
    // Another effect has none either.
    let filter = h.add_effect(0, "Filter");
    h.sync();
    let position = h.chain_position(0, filter);
    h.act(&format!(
        "(table-editor-open! (nth (let ((t (track 0))) t.devices) {position}))"
    ));
    assert!(h.error().contains("not a Filter Table"), "{}", h.error());
    assert_eq!(ui().node_id, node);

    h.act("(table-editor-close!)");
    assert!(fte::session_ui_state().is_none());
    close_session();
}

#[test]
fn the_device_goes_nil_when_the_session_closes_or_its_node_is_deleted() {
    let _lock = te_lock();
    let mut h = Harness::new();
    close_session();
    let (slot, _) = h.track_filter_table();
    observe(&mut h);
    h.act("(table-editor-open! ft)");
    assert_eq!(h.eval_te("(= te.device ft)"), Value::Bool(true));
    h.act("(table-editor-close!)");
    assert_eq!(h.te_cells()[0], Value::Bool(false));
    assert_eq!(h.eval_te("te.device"), Value::Nil);

    h.act("(table-editor-open! ft)");
    assert_eq!(h.eval_te("(= te.device ft)"), Value::Bool(true));
    let held = h.eval_te("ft");
    h.app
        .graph_controller()
        .delete_custom_effect_slot(0, slot)
        .expect("delete");
    h.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert!(
        fte::session_ui_state().is_none(),
        "the session went with its node"
    );
    let Value::Instance(held) = held else {
        panic!("not an instance");
    };
    assert!(!h.rt().instance_is_live(held));
    let cells = h.te_cells();
    assert_eq!(cells[0], Value::Bool(false));
    assert_eq!(h.eval_te("te.device"), Value::Nil);
    let id = h.singleton(TABLE_EDITOR);
    assert_eq!(
        h.rt().instance_field(id, "device").expect("device"),
        Value::Nil
    );
    close_session();
}
