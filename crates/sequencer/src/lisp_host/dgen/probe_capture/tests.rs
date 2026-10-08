//! Probe capture tests. Every test mutates process-global registries and the
//! watch gate; nextest runs each in its own process.

use super::*;
use crate::lisp_host::parse_manifest;

fn probe_manifest() -> DGenManifest {
    parse_manifest(
        r#"{"processAbi": "dgen-host-abi-v1",
            "outputs": [{"channel": 0, "name": "audio"}],
            "probes": [
              {"id": "cut", "occurrence": 0, "channel": 1, "view": "scope", "name": null},
              {"id": "env", "occurrence": 0, "channel": 2, "view": "number"},
              {"id": "env", "occurrence": 1, "channel": 3, "view": "number"}
            ]}"#,
    )
    .expect("manifest parses")
}

/// Output buffers for `probe_manifest`: audio, then one probe per channel.
struct Outputs {
    buffers: Vec<Vec<f32>>,
    pointers: Vec<*mut f32>,
}

impl Outputs {
    fn new(channels: Vec<Vec<f32>>) -> Self {
        let mut buffers = channels;
        let pointers = buffers
            .iter_mut()
            .map(|buffer| buffer.as_mut_ptr())
            .collect();
        Self { buffers, pointers }
    }

    fn frames(&self) -> i32 {
        self.buffers[0].len() as i32
    }
}

fn block(cut: &[f32], env: f32, env1: f32) -> Outputs {
    let frames = cut.len();
    Outputs::new(vec![
        vec![0.0; frames],
        cut.to_vec(),
        vec![env; frames],
        vec![env1; frames],
    ])
}

const FN_A: usize = 0x1000;
const FN_B: usize = 0x2000;

fn record_voice(engine_id: usize, voice_idx: usize, process_fn: usize, outputs: &Outputs) {
    unsafe {
        record_dgen_voice_probes(
            engine_id,
            voice_idx,
            process_fn,
            outputs.pointers.as_ptr(),
            outputs.frames(),
        )
    };
}

fn instrument(engine_id: usize) -> ProbeInstance {
    ProbeInstance::Instrument { engine_id }
}

#[test]
fn summary_tracks_last_min_max_and_sequence_per_block() {
    let engine = 5;
    publish_dgen_instrument_probes(engine, &probe_manifest(), FN_A);
    let _watch = ProbeWatchGuard::new();
    assert_eq!(
        probe_snapshot(instrument(engine), "cut", 0),
        None,
        "no block captured yet"
    );

    record_voice(engine, 0, FN_A, &block(&[0.5, -2.0, 3.0, 1.25], 0.7, -0.3));
    assert_eq!(
        probe_snapshot(instrument(engine), "cut", 0),
        Some(ProbeReading {
            last: 1.25,
            min: -2.0,
            max: 3.0,
            seq: 1
        })
    );
    // Repeated ids resolve by occurrence.
    assert_eq!(
        probe_snapshot(instrument(engine), "env", 0).unwrap().last,
        0.7
    );
    assert_eq!(
        probe_snapshot(instrument(engine), "env", 1).unwrap().last,
        -0.3
    );
    assert_eq!(probe_snapshot(instrument(engine), "env", 2), None);

    // A block's summary replaces the previous one; it does not accumulate.
    record_voice(engine, 0, FN_A, &block(&[0.1, 0.2], 0.0, 0.0));
    assert_eq!(
        probe_snapshot(instrument(engine), "cut", 0),
        Some(ProbeReading {
            last: 0.2,
            min: 0.1,
            max: 0.2,
            seq: 2
        })
    );
    assert_eq!(
        probe_infos(instrument(engine))
            .iter()
            .map(|info| (
                info.id.as_str(),
                info.occurrence,
                info.channel,
                info.view.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("cut", 0, 1, "scope"),
            ("env", 0, 2, "number"),
            ("env", 1, 3, "number")
        ]
    );
}

#[test]
fn scope_decimates_each_block_to_min_max_pairs() {
    let engine = 6;
    publish_dgen_instrument_probes(engine, &probe_manifest(), FN_A);
    let _watch = ProbeWatchGuard::new();
    // 16 frames → 8 chunks of 2: chunk k holds (k, -k).
    let cut: Vec<f32> = (0..8).flat_map(|k| [k as f32, -(k as f32)]).collect();
    record_voice(engine, 0, FN_A, &block(&cut, 0.0, 0.0));
    let mut window = Vec::new();
    assert!(probe_scope_window(
        instrument(engine),
        "cut",
        0,
        64,
        &mut window
    ));
    let expected: Vec<(f32, f32)> = (0..8).map(|k| (-(k as f32), k as f32)).collect();
    assert_eq!(window, expected);

    // A block shorter than the point budget contributes one pair per frame.
    record_voice(engine, 0, FN_A, &block(&[9.0, 10.0, 11.0], 0.0, 0.0));
    assert!(probe_scope_window(
        instrument(engine),
        "cut",
        0,
        3,
        &mut window
    ));
    assert_eq!(window, vec![(9.0, 9.0), (10.0, 10.0), (11.0, 11.0)]);

    // `number` probes carry no ring.
    assert!(!probe_scope_window(
        instrument(engine),
        "env",
        0,
        8,
        &mut window
    ));
    assert!(window.is_empty());
}

#[test]
fn scope_ring_wraps_and_returns_the_most_recent_pairs_in_order() {
    let ring = ScopeRing::new();
    let blocks = PROBE_SCOPE_RING_PAIRS / PROBE_SCOPE_POINTS_PER_BLOCK + 3;
    for block in 0..blocks {
        let samples: Vec<f32> = (0..PROBE_SCOPE_POINTS_PER_BLOCK)
            .map(|point| (block * PROBE_SCOPE_POINTS_PER_BLOCK + point) as f32)
            .collect();
        ring.push_block(&samples);
    }
    let total = blocks * PROBE_SCOPE_POINTS_PER_BLOCK;
    let mut window = Vec::new();
    ring.read_recent(usize::MAX, &mut window);
    let readable = PROBE_SCOPE_RING_PAIRS - PROBE_SCOPE_POINTS_PER_BLOCK;
    assert_eq!(
        window.len(),
        readable,
        "capped at the ring's capacity less the block a producer may be writing"
    );
    let expected: Vec<(f32, f32)> = (total - readable..total)
        .map(|value| (value as f32, value as f32))
        .collect();
    assert_eq!(window, expected);

    ring.read_recent(5, &mut window);
    assert_eq!(window.first().unwrap().0, (total - 5) as f32);
    assert_eq!(window.last().unwrap().0, (total - 1) as f32);
}

#[test]
fn closed_gate_skips_capture() {
    let engine = 7;
    publish_dgen_instrument_probes(engine, &probe_manifest(), FN_A);
    assert!(!probes_watched());
    record_voice(engine, 0, FN_A, &block(&[1.0], 0.0, 0.0));
    assert_eq!(probe_snapshot(instrument(engine), "cut", 0), None);

    {
        let _watch = ProbeWatchGuard::new();
        watch_probes();
        assert!(probes_watched());
        unwatch_probes();
        assert!(probes_watched(), "the guard still holds the gate open");
        record_voice(engine, 0, FN_A, &block(&[2.0], 0.0, 0.0));
    }
    assert!(!probes_watched());
    record_voice(engine, 0, FN_A, &block(&[3.0], 0.0, 0.0));
    assert_eq!(
        probe_snapshot(instrument(engine), "cut", 0).unwrap().last,
        2.0
    );

    unwatch_probes();
    assert!(!probes_watched(), "an unmatched unwatch saturates at zero");
}

#[test]
fn only_the_most_recently_allocated_voice_feeds_capture() {
    let engine = 8;
    publish_dgen_instrument_probes(engine, &probe_manifest(), FN_A);
    let _watch = ProbeWatchGuard::new();
    assert_eq!(dgen_display_voice(engine), Some(0));

    note_dgen_display_voice(engine, 2);
    record_voice(engine, 0, FN_A, &block(&[1.0], 0.0, 0.0));
    record_voice(engine, 2, FN_A, &block(&[2.0], 0.0, 0.0));
    record_voice(engine, 3, FN_A, &block(&[3.0], 0.0, 0.0));
    let reading = probe_snapshot(instrument(engine), "cut", 0).unwrap();
    assert_eq!((reading.last, reading.seq), (2.0, 1));

    note_dgen_display_voice(engine, 3);
    record_voice(engine, 2, FN_A, &block(&[4.0], 0.0, 0.0));
    record_voice(engine, 3, FN_A, &block(&[5.0], 0.0, 0.0));
    assert_eq!(
        probe_snapshot(instrument(engine), "cut", 0).unwrap().last,
        5.0
    );

    // Out-of-range ids are ignored rather than indexing past the tables.
    note_dgen_display_voice(MAX_INSTRUMENT_ENGINES, 0);
    note_dgen_display_voice(engine, MAX_VOICES);
    assert_eq!(dgen_display_voice(engine), Some(3));
}

#[test]
fn rebuild_retires_the_old_set_until_no_reader_holds_it() {
    let engine = 9;
    publish_dgen_instrument_probes(engine, &probe_manifest(), FN_A);
    let _watch = ProbeWatchGuard::new();

    let reader = ReaderGuard::enter();
    publish_dgen_instrument_probes(engine, &probe_manifest(), FN_B);
    assert_eq!(
        registry().retired.len(),
        1,
        "a reader may still hold the old set"
    );
    reclaim_retired_probe_sets();
    assert_eq!(registry().retired.len(), 1);
    drop(reader);
    reclaim_retired_probe_sets();
    assert_eq!(registry().retired.len(), 0);

    // A stale node still rendering the old library cannot write the new set.
    record_voice(engine, 0, FN_A, &block(&[1.0], 0.0, 0.0));
    assert_eq!(probe_snapshot(instrument(engine), "cut", 0), None);
    record_voice(engine, 0, FN_B, &block(&[2.0], 0.0, 0.0));
    assert_eq!(
        probe_snapshot(instrument(engine), "cut", 0).unwrap().last,
        2.0
    );

    // Recompiling without probes clears the instance.
    let plain =
        parse_manifest(r#"{"processAbi": "dgen-host-abi-v1", "outputs": [{"channel": 0}]}"#)
            .unwrap();
    publish_dgen_instrument_probes(engine, &plain, FN_A);
    assert!(INSTRUMENT_PROBE_SETS[engine]
        .load(Ordering::SeqCst)
        .is_null());
    assert_eq!(probe_snapshot(instrument(engine), "cut", 0), None);
    assert!(probe_infos(instrument(engine)).is_empty());
}

#[test]
fn effect_probe_tokens_follow_node_lifetime() {
    let node_id = 41;
    let instance = ProbeInstance::Effect { node_id };
    let token = register_effect_probes(&probe_manifest(), FN_A).expect("token");
    let header = token.header_code();
    assert!(header < 0.0);
    assert_eq!(decode_effect_probe_token(header), Some((token.token, token.generation)));
    assert_eq!(
        decode_effect_probe_token(17.0),
        None,
        "slot ids are not tokens"
    );
    bind_effect_probe_node(token, node_id);

    let _watch = ProbeWatchGuard::new();
    let outputs = block(&[0.25, -0.5], 1.0, 2.0);
    unsafe {
        record_dgen_effect_probes(header, FN_A, outputs.pointers.as_ptr(), outputs.frames());
        // A header without a token never captures.
        record_dgen_effect_probes(3.0, FN_A, outputs.pointers.as_ptr(), outputs.frames());
    }
    assert_eq!(
        probe_snapshot(instance, "cut", 0),
        Some(ProbeReading {
            last: -0.5,
            min: -0.5,
            max: 0.25,
            seq: 1
        })
    );

    // While the node lives its token stays taken.
    let other = register_effect_probes(&probe_manifest(), FN_B).expect("second token");
    assert_ne!(other, token);
    release_effect_probe_token(other);

    clear_effect_probes(node_id);
    assert!(EFFECT_PROBE_SETS[token.token].load(Ordering::SeqCst).is_null());
    assert_eq!(probe_snapshot(instance, "cut", 0), None);
    // A late block from the deleted node is dropped.
    unsafe {
        record_dgen_effect_probes(header, FN_A, outputs.pointers.as_ptr(), outputs.frames());
    }
    assert_eq!(registry().effects.len(), 0);

    let plain =
        parse_manifest(r#"{"processAbi": "dgen-host-abi-v1", "outputs": [{"channel": 0}]}"#)
            .unwrap();
    assert_eq!(
        register_effect_probes(&plain, FN_A),
        None,
        "no probes, no token"
    );
}

/// Compiles a real instrument with `probe` taps and renders one block
/// through the live instrument wrapper. Needs the pinned DGenLisp
/// (`content/dgenlisp.lock`, v0.1.32+ knows `probe`) or `ESEQ_DGENLISP_TOOL`.
#[test]
fn compiled_instrument_probe_reaches_capture_through_the_wrapper() {
    let source = r#"
(def gate (in 1 @name gate))
(out (+ 0.1 (* 0.1 (probe (+ 0.25 gate) @id "g" @view scope))) 1 @name audio)
(probe 0.75 @id "k")
"#;
    let compiled = crate::lisp_host::compile_and_load_instrument_uncached_with_asset_base(
        source, 48_000, None,
    )
    .expect("compile probe instrument");
    let manifest = &compiled.manifest;
    assert_eq!(
        manifest
            .probes
            .iter()
            .map(|probe| probe.id.as_str())
            .collect::<Vec<_>>(),
        vec!["g", "k"],
        "the configured compiler must emit manifest probes[]"
    );
    assert_eq!(manifest.audio_output_channels(), vec![0]);

    let engine_id = 11;
    let voice_idx = 0;
    let slot_id = engine_id * MAX_VOICES + voice_idx;
    let process_fn = compiled.lib.process_fn;
    crate::lisp_host::set_dgen_instrument_fn(slot_id, process_fn);
    crate::lisp_host::set_dgen_instrument_output_count(slot_id, manifest.n_outputs.max(1));
    crate::lisp_host::set_dgen_engine_enabled_voices(engine_id, 1);
    publish_dgen_instrument_probes(engine_id, manifest, process_fn as usize);
    note_dgen_display_voice(engine_id, voice_idx);

    let init = crate::lisp_host::build_init_message_for_voice(slot_id, manifest, voice_idx);
    let mut state =
        vec![0.0_f32; crate::lisp_host::dgen_total_state_slots(manifest.total_memory_slots)];
    unsafe {
        crate::lisp_host::dgenlisp_init(
            state.as_mut_ptr().cast(),
            48_000,
            128,
            init.as_ptr().cast(),
        );
    }

    let frames = 128;
    let mut inputs = vec![vec![1.0_f32; frames]; manifest.n_inputs.max(1)];
    let input_ptrs: Vec<*mut f32> = inputs.iter_mut().map(|input| input.as_mut_ptr()).collect();
    let mut outputs = vec![vec![0.0_f32; frames]; manifest.n_outputs.max(1)];
    let output_ptrs: Vec<*mut f32> = outputs
        .iter_mut()
        .map(|output| output.as_mut_ptr())
        .collect();
    let process = crate::lisp_host::dgenlisp_instrument_vtable()
        .process
        .unwrap();

    let render = |state: &mut Vec<f32>| unsafe {
        process(
            input_ptrs.as_ptr(),
            output_ptrs.as_ptr(),
            frames as i32,
            state.as_mut_ptr().cast(),
            std::ptr::null_mut(),
        );
    };

    // Gate closed: the block renders but nothing is captured.
    render(&mut state);
    assert_eq!(probe_snapshot(instrument(engine_id), "k", 0), None);

    let _watch = ProbeWatchGuard::new();
    render(&mut state);
    let audio = unsafe { std::slice::from_raw_parts(output_ptrs[0], frames) };
    assert!(
        (audio[frames - 1] - 0.225).abs() < 1e-5,
        "audio: {}",
        audio[frames - 1]
    );

    let k = probe_snapshot(instrument(engine_id), "k", 0).expect("k captured");
    assert_eq!((k.last, k.min, k.max, k.seq), (0.75, 0.75, 0.75, 1));
    let g = probe_snapshot(instrument(engine_id), "g", 0).expect("g captured");
    assert!(
        (g.last - 1.25).abs() < 1e-6 && (g.min - 1.25).abs() < 1e-6,
        "{g:?}"
    );
    let mut window = Vec::new();
    assert!(probe_scope_window(
        instrument(engine_id),
        "g",
        0,
        64,
        &mut window
    ));
    assert_eq!(window.len(), PROBE_SCOPE_POINTS_PER_BLOCK);
    assert!(window
        .iter()
        .all(|&(lo, hi)| (lo - 1.25).abs() < 1e-6 && (hi - 1.25).abs() < 1e-6));
}

/// Teardown only queues a node's deletion, so an effect's token can be
/// reallocated while the old node still renders. The generation in the
/// header code keeps that stale node out of its successor's slots.
#[test]
fn reused_effect_probe_token_ignores_the_stale_node() {
    let old_node = 51;
    let new_node = 52;
    let old = register_effect_probes(&probe_manifest(), FN_A).expect("old token");
    bind_effect_probe_node(old, old_node);
    clear_effect_probes(old_node);

    // Same compiled library (two instances of one effect): the process-fn
    // check alone cannot tell the nodes apart.
    let new = register_effect_probes(&probe_manifest(), FN_A).expect("new token");
    bind_effect_probe_node(new, new_node);
    assert_eq!(new.token, old.token, "the freed token is reused");
    assert_ne!(new.generation, old.generation);
    assert_ne!(new.header_code(), old.header_code());

    let _watch = ProbeWatchGuard::new();
    let stale = block(&[9.0, 9.0], 9.0, 9.0);
    let live = block(&[0.5, 0.25], 1.0, 2.0);
    unsafe {
        // The old node renders one more block before its delete applies.
        record_dgen_effect_probes(old.header_code(), FN_A, stale.pointers.as_ptr(), stale.frames());
    }
    let instance = ProbeInstance::Effect { node_id: new_node };
    assert_eq!(probe_snapshot(instance, "cut", 0), None, "stale node captured nothing");
    unsafe {
        record_dgen_effect_probes(new.header_code(), FN_A, live.pointers.as_ptr(), live.frames());
        record_dgen_effect_probes(old.header_code(), FN_A, stale.pointers.as_ptr(), stale.frames());
    }
    assert_eq!(
        probe_snapshot(instance, "cut", 0),
        Some(ProbeReading { last: 0.25, min: 0.25, max: 0.5, seq: 1 })
    );
    clear_effect_probes(new_node);
}

#[test]
fn effect_probe_codes_round_trip_at_the_f32_exact_limit() {
    let last_token = MAX_EFFECT_PROBE_INSTANCES - 1;
    let last_generation = EFFECT_PROBE_GENERATIONS - 1;
    for (token, generation) in [(0, 0), (3, 1), (last_token, last_generation)] {
        let code = encode_effect_probe_token(token, generation);
        assert_eq!(decode_effect_probe_token(code), Some((token, generation)));
    }
    // Generations wrap rather than overflow the exact range.
    assert_eq!(
        decode_effect_probe_token(encode_effect_probe_token(2, EFFECT_PROBE_GENERATIONS + 4)),
        Some((2, 4))
    );
    for not_a_code in [0.0, 17.0, -0.5, f32::NAN] {
        assert_eq!(decode_effect_probe_token(not_a_code), None, "{not_a_code}");
    }
}

/// A pair the producer may be rewriting is never returned: the producer
/// writes a block's pairs before it publishes `written`, so the oldest
/// `PROBE_SCOPE_POINTS_PER_BLOCK` pairs of a full ring are suspect.
#[test]
fn scope_ring_read_never_returns_pairs_the_producer_may_be_writing() {
    let ring = ScopeRing::new();
    let blocks = PROBE_SCOPE_RING_PAIRS / PROBE_SCOPE_POINTS_PER_BLOCK + 3;
    for block in 0..blocks {
        ring.push_block(&[block as f32; PROBE_SCOPE_POINTS_PER_BLOCK]);
    }
    // Simulate a producer mid-block: the next pairs' slots (the oldest
    // pairs of the window) are rewritten, `written` not yet advanced.
    let written = ring.written.load(Ordering::Relaxed);
    for point in 0..PROBE_SCOPE_POINTS_PER_BLOCK {
        let index = (written as usize + point) & (PROBE_SCOPE_RING_PAIRS - 1);
        ring.mins[index].store((-1.0f32).to_bits(), Ordering::Relaxed);
        ring.maxs[index].store(f32::NAN.to_bits(), Ordering::Relaxed);
    }
    let mut out = Vec::new();
    ring.read_recent(PROBE_SCOPE_RING_PAIRS, &mut out);
    assert_eq!(out.len(), PROBE_SCOPE_RING_PAIRS - PROBE_SCOPE_POINTS_PER_BLOCK);
    assert!(out.iter().all(|&(min, max)| min >= 0.0 && min == max), "torn pair in {out:?}");
    assert_eq!(out.last(), Some(&((blocks - 1) as f32, (blocks - 1) as f32)));
    // A window short of the suspect pairs loses nothing.
    ring.read_recent(16, &mut out);
    assert_eq!(out.len(), 16);
}

#[test]
fn parse_manifest_reads_probe_views_and_capture_skips_id_less_probes() {
    let manifest = parse_manifest(
        r#"{"processAbi": "dgen-host-abi-v1",
            "outputs": [{"channel": 0}],
            "probes": [
              {"id": "", "occurrence": 0, "channel": 1, "view": "scope"},
              {"id": "k", "occurrence": 0, "channel": 2, "view": "scope"},
              {"id": "m", "occurrence": 0, "channel": 3, "view": "meter"},
              {"id": "n", "occurrence": 0, "channel": 4},
              {"id": "z", "occurrence": 0, "channel": 5, "view": "spectrum"}
            ]}"#,
    )
    .expect("manifest parses");
    assert_eq!(
        manifest.probes.iter().map(|probe| probe.view).collect::<Vec<_>>(),
        vec![
            ProbeView::Scope,
            ProbeView::Scope,
            ProbeView::Meter,
            ProbeView::Number,
            ProbeView::Number
        ],
        "a missing or unknown view reads as a number"
    );
    // The id-less tap is still display signal, never audio.
    assert_eq!(manifest.audio_output_channels(), vec![0]);

    let engine = 9;
    publish_dgen_instrument_probes(engine, &manifest, FN_A);
    assert_eq!(
        probe_infos(instrument(engine))
            .iter()
            .map(|probe| probe.id.as_str())
            .collect::<Vec<_>>(),
        vec!["k", "m", "n", "z"],
        "nothing can address an id-less probe"
    );
    let set = probe_set(instrument(engine)).expect("set");
    let mut window = Vec::new();
    assert!(set.read_scope(0, 8, &mut window), "k is a scope");
    assert!(!set.read_scope(1, 8, &mut window), "m is not");
    clear_dgen_instrument_probes(engine);
}
