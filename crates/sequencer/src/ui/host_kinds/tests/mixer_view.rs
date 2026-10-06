//! The factory mixer, its legacy predecessor and the MIDImix map, ported to
//! the kinds (kind-bindings spec §13 stage 8, eseq-0l17.13).

use super::views::{assert_ported, distro, instance_bindings, legacy_forms};
use super::*;

/// The ported files' sources.
const PORTED: [(&str, &str); 3] = [
    (
        "ui/mixer.lisp",
        include_str!("../../../../../../content/ui/mixer.lisp"),
    ),
    (
        "ui/legacy/mixer.lisp",
        include_str!("../../../../../../content/ui/legacy/mixer.lisp"),
    ),
    (
        "ui/midi-midimix.lisp",
        include_str!("../../../../../../content/ui/midi-midimix.lisp"),
    ),
];

#[test]
fn ported_mixer_views_use_no_legacy_binding_forms() {
    // The views refer their kinds; the MIDImix map reaches them through
    // the mixer's render items.
    assert_ported(&PORTED[..2]);
    let (file, source) = PORTED[2];
    assert_eq!(legacy_forms(source), Vec::<&str>::new(), "{file}");
}

#[test]
fn the_mixer_binds_its_host_state_through_kinds_and_only_repaints_while_mixing() {
    let mut h = distro();
    let (tree, revision) = h.buffer_tree("*mixer*");
    let mut bound = Vec::new();
    instance_bindings(&tree, &mut bound, &mut Vec::new());
    let t0 = h.track_id(0);
    for field in [
        "volume",
        "peak",
        "pan",
        "audible",
        "soloed",
        "armed",
        "in-selection",
        "delete-target",
    ] {
        assert!(
            bound.contains(&(t0, field.to_string())),
            "track 0's {field} is bound: {bound:?}"
        );
    }
    let buses = h.eval_all("(buses)");
    let buses = h.instances(buses);
    let master = h.singleton(MASTER);
    for field in ["peak-l", "peak-r"] {
        assert!(
            bound.contains(&(master, field.to_string())),
            "the main mix meters master.{field}"
        );
    }
    for &bus in &buses[1..] {
        for field in ["volume", "peak", "muted", "soloed"] {
            assert!(
                bound.contains(&(bus, field.to_string())),
                "bus {bus}'s {field} is bound"
            );
        }
    }
    let sends = h.eval_all("(let ((t (track 0))) t.sends)");
    for send in h.instances(sends) {
        for field in ["display", "locked", "amount"] {
            assert!(
                bound.contains(&(send, field.to_string())),
                "send {send}'s {field} is bound"
            );
        }
    }

    // Mixing and playing: faders, pan, mute, solo, arm and the meters
    // repaint; the mixer never re-renders for them.
    {
        let params = &h.shared.state.pattern.track_params[0];
        params.set_volume(0.3);
        params.set_pan(-0.5);
        params.set_mute(true);
    }
    h.shared.state.pattern.track_params[1].set_solo(true);
    h.app.buses[1].volume = 0.25;
    h.app.buses[1].mute = true;
    h.meters.cached_track_peak_levels = vec![0.5; h.app.tracks.len()];
    h.meters.cached_bus_peak_levels = vec![0.375; h.app.buses.len()];
    h.meters.cached_peak_l_level = 0.25;
    h.meters.cached_peak_r_level = 0.5;
    assert!(h.sync());
    h.show_all();
    h.editor.refresh_runtime_side_effects();
    assert_eq!(
        h.eval_all("(let ((t (track 0))) t.volume)"),
        Value::Number(0.3_f32 as f64)
    );
    assert_eq!(
        h.eval_all("(let ((t (track 0))) t.audible)"),
        Value::Bool(false)
    );
    assert_eq!(
        h.eval_all("(let ((b (nth (buses) 1))) b.muted)"),
        Value::Bool(true)
    );
    assert_eq!(h.eval_all("master.peak-r"), Value::Number(0.5));
    assert_eq!(
        h.buffer_tree("*mixer*").1,
        revision,
        "mixing and metering only repaint"
    );
}

#[test]
fn the_selection_highlight_and_the_group_delete_target_are_kind_fields() {
    let mut h = Harness::new();
    h.app.group_tracks_recorded(vec![0, 1]).expect("group");
    h.share_buses_and_groups();
    h.sync();
    h.eval_all(
        "(def t0 (track 0)) (def t1 (track 1)) (def sel0 #'t0.in-selection)
         (def sel1 #'t1.in-selection) (def g (first (groups))) (def gdel #'g.delete-target)",
    );
    h.sync();
    // The current track, and the multi-selection beside it.
    assert_eq!(h.slot("sel0"), 1.0);
    assert_eq!(h.slot("sel1"), 0.0);
    h.shared.selected_tracks.lock().unwrap().insert(1);
    h.sync();
    assert_eq!(h.slot("sel1"), 1.0);
    // The group as the mixer's delete target, as the host arms it.
    assert_eq!(h.slot("gdel"), 0.0);
    let gid = h.app.groups[0].id;
    let target = |h: &Harness| h.shared.active_delete_target.lock().unwrap().clone();
    // A host write moves the target's version (the field follows only it).
    let arm = |h: &mut Harness, target: Option<ActiveDeleteTarget>| {
        *h.shared.active_delete_target.lock().unwrap() = target;
        (h.shared.active_delete_target_version).fetch_add(1, Ordering::Relaxed);
    };
    arm(&mut h, Some(ActiveDeleteTarget::MixerGroup { group_id: gid }));
    h.sync();
    assert_eq!(h.slot("gdel"), 1.0);
    arm(&mut h, None);
    h.sync();
    assert_eq!(h.slot("gdel"), 0.0);
    assert!(h.computed(f::GROUP_DELETE_TARGET) > 0);
    // Through the field's setter: applied at once (UI-thread state).
    h.eval_all("(set! g.delete-target true)");
    assert_eq!(
        target(&h),
        Some(ActiveDeleteTarget::MixerGroup { group_id: gid })
    );
    h.sync();
    assert_eq!(h.slot("gdel"), 1.0);
    h.eval_all("(set! g.delete-target false)");
    assert_eq!(target(&h), None);
    // Clearing a group that is not the target leaves the armed one.
    let other = Some(ActiveDeleteTarget::MixerGroup { group_id: gid + 1 });
    *h.shared.active_delete_target.lock().unwrap() = other.clone();
    h.eval_all("(set! g.delete-target false)");
    assert_eq!(target(&h), other);
}

#[test]
fn a_send_shows_the_level_a_process_writes() {
    let mut h = Harness::new();
    let bus = h.app.buses[1].id;
    h.sync();
    h.eval_all(&format!(
        "(def s (first (filter (lambda (s) (= s.bus.bid {})) (let ((t (track 0))) t.sends))))
         (def mapped #'s.process-mapped) (def written #'s.process-value) (def shown #'s.display)",
        bus.0
    ));
    h.sync();
    assert_eq!(h.slot("mapped"), 0.0);
    assert_eq!(
        h.slot("written"),
        h.slot("shown"),
        "unmapped: the shown level"
    );
    let send = sequencer::process::ParamTarget::BusSend { bus: bus.0 };
    let chain = one_slot_chain(send, true);
    assert!(h.shared.state.set_track_process_chain(0, chain));
    h.shared
        .ui_invalidations
        .push(UiInvalidation::ProcessChain { track: 0 });
    h.sync();
    assert_eq!(h.slot("mapped"), 1.0);
    h.shared.state.publish_process_effective_sends(
        0,
        &[sequencer::process::ProcessEffectiveSend {
            bus: bus.0,
            base: 0.0,
            value: 0.75,
            clamped: false,
        }],
    );
    h.sync();
    assert_eq!(h.slot("written"), 0.75);
}
