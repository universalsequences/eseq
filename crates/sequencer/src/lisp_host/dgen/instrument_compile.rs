/*!
Instrument-flavored side of the DGenLisp compile pipeline.

Instruments share `effect_compile`'s machinery but get their own injected
preamble (`INSTRUMENT_PREAMBLE`: gate/pitch/velocity plumbing, `def-voice`
helpers) and voice-count handling. Entry points mirror the effect ones
(`compile_instrument*`, `compile_and_load_instrument*`). Also contains the
offline render harness used by tests and the audition tool
(`render_instrument_source_for_test`, `render_loaded_effect_for_test`, ...),
compile-failure logging, and `INSTRUMENT_TEMPLATE` for newly created
instruments.
*/

use super::super::*;

pub(in crate::lisp_host) const INSTRUMENT_PREAMBLE: &str = r#"; Shared instrument helpers injected at compile time.
; `samplerate` is provided by DGenLisp as runtime host sample-rate context.

(defmacro mod_unipolar (m)
  (* (+ m 1.0) 0.5))

(defmacro apply_pitch_mod_semi (base_hz mod amt_semi)
  (def ln2 (log 2))
  (* base_hz (exp (* ln2 (/ (* mod amt_semi) 12)))))

(defmacro apply_cutoff_mod_safe (base mod amt)
  (min 11000 (max 60 (+ base (* mod amt)))))

(defmacro apply_pw_mod_safe (base mod amt)
  (clip (+ base (* mod amt)) 0.03 0.97))

; PolyBLEP transition correction for anti-aliased hard edges.
; Kept with a polypleb alias because that typo is memorable and fun.
(defmacro polyblep (phase freq)
  (def dt (clip (/ freq samplerate) 0.000001 0.5))
  (def left_x (/ phase dt))
  (def left (+ (- (* 2.0 left_x) (* left_x left_x)) -1.0))
  (def right_x (/ (- phase 1.0) dt))
  (def right (+ (* right_x right_x) (* 2.0 right_x) 1.0))
  (+ (* (lt phase dt) left)
     (* (gt phase (- 1.0 dt)) right)))

(defmacro polypleb (phase freq)
  (polyblep phase freq))

(defmacro polyblep_saw (phase freq)
  (- (scale phase 0 1 -1 1)
     (polyblep phase freq)))

(defmacro polyblep_pulse (phase width freq)
  (def w (clip width 0.01 0.99))
  (def falling_phase (wrap (- phase w) 0 1))
  (+ (scale (lt phase w) 0 1 -1 1)
     (polyblep phase freq)
     (* -1.0 (polyblep falling_phase freq))))

; Wavetable helpers assume tensor shape [samples, waves]. sample is shape-aware
; (normalized phase scaled by the table's row count at compile time).
(defmacro wavetable-read (table wave phase)
  (sample table phase wave))

(defmacro wavetable-morph (table wave_a wave_b phase morph)
  (wavetable-read table (+ wave_a (* (clip morph 0 1) (- wave_b wave_a))) phase))

; Deprecated aliases (baked 512-row assumption no longer needed):
(defmacro wavetable-read-512 (table wave phase)
  (wavetable-read table wave phase))

(defmacro wavetable-morph-512 (table wave_a wave_b phase morph)
  (wavetable-morph table wave_a wave_b phase morph))

; Cytomic-style ZDF state variable filter.
; cutoff in Hz, q is resonance (0.5 = no resonance, higher = more).
; mode: 0=LP, 1=BP, 2=HP, 3=notch, 4=peak, 5=allpass.
(defmacro svf (input cutoff q mode)
  (def safe_cutoff (clip cutoff 1.0 (* samplerate 0.49)))
  (def safe_q (max q 0.001))
  (def g (tan (* pi (/ safe_cutoff samplerate))))
  (def k (/ 1.0 safe_q))
  (def a1 (/ 1.0 (+ 1.0 (* g (+ g k)))))
  (def a2 (* g a1))
  (def a3 (* g a2))

  (make-history ic1eq)
  (make-history ic2eq)

  (def ic1 (read-history ic1eq))
  (def ic2 (read-history ic2eq))
  (def v3 (- input ic2))
  (def v1 (+ (* a1 ic1) (* a2 v3)))
  (def v2 (+ ic2 (* a2 ic1) (* a3 v3)))

  (write-history ic1eq (- (* 2.0 v1) ic1))
  (write-history ic2eq (- (* 2.0 v2) ic2))

  (def lp v2)
  (def bp v1)
  (def hp (- input (* k v1) v2))
  (def notch (+ hp lp))
  (def peak (- lp hp))
  (def ap (- notch (* k v1)))

  (+ (* (eq mode 0) lp)
     (* (eq mode 1) bp)
     (* (eq mode 2) hp)
     (* (eq mode 3) notch)
     (* (eq mode 4) peak)
     (* (eq mode 5) ap)))

; ZDF Moog ladder filter, 4-pole, with input drive, tanh feedback saturation,
; and resonance-proportional passband gain compensation.
; cutoff in Hz, res is 0..1, drive pre-saturates the input.
(defmacro ladder (input cutoff res drive)
  (def wd (* twopi cutoff))
  (def T (/ 1 samplerate))
  (def wa (* (/ 2.0 T) (tan (* wd T 0.5))))
  (def g (* wa T 0.5))
  (def G (/ g (+ 1 g)))
  (def G4 (* G G G G))
  (def k (* res 4))

  (def fb_trim 0.5)

  (make-history z1)
  (make-history z2)
  (make-history z3)
  (make-history z4)

  (def hz1 (read-history z1))
  (def hz2 (read-history z2))
  (def hz3 (read-history z3))
  (def hz4 (read-history z4))
  (def inv_1pg (/ 1 (+ 1 g)))
  (def S (+ (* hz1 G G G inv_1pg)
            (* hz2 G G inv_1pg)
            (* hz3 G inv_1pg)
            (* hz4 inv_1pg)))

  (def driven_input (tanh (* drive input)))
  (def u (/ (- driven_input (* k fb_trim S))
            (+ 1 (* k fb_trim G4))))
  (def x1 (- u (* k (tanh (* fb_trim (+ (* G4 u) S))))))

  (def v1 (* (- x1 hz1) G))
  (def y1 (+ v1 hz1))
  (write-history z1 (+ y1 v1))

  (def v2 (* (- y1 hz2) G))
  (def y2 (+ v2 hz2))
  (write-history z2 (+ y2 v2))

  (def v3 (* (- y2 hz3) G))
  (def y3 (+ v3 hz3))
  (write-history z3 (+ y3 v3))

  (def v4 (* (- y3 hz4) G))
  (def y4 (+ v4 hz4))
  (write-history z4 (+ y4 v4))

  (+ y4 (* res 0.0013 input)))

(defmacro adsr (gate_sig trigger_sig attack_ms decay_ms sustain release_ms)
  (make-history env)
  (make-history gate_hist)
  (make-history stage_hist)

  ; Retriggers first fade any leftover voice history to silence over a
  ; short de-click window, then start a linear attack from near zero.
  ; Decay/release are one-pole curves scaled to settle near the target
  ; over the requested number of milliseconds.
  (def sr samplerate)
  (def env_time_scale 6.907755)
  (def reset_samples (* 0.003 sr))
  (def attack_samples (max 1.0 (* attack_ms 0.001 sr)))
  (def decay_samples (max 1.0 (* decay_ms 0.001 sr)))
  (def release_samples (max 1.0 (* release_ms 0.001 sr)))
  (def reset_coeff (- 1.0 (exp (/ (* -1.0 env_time_scale) reset_samples))))
  (def decay_coeff (- 1.0 (exp (/ (* -1.0 env_time_scale) decay_samples))))
  (def release_coeff (- 1.0 (exp (/ (* -1.0 env_time_scale) release_samples))))

  (def prev_env (read-history env))
  (def prev_gate (read-history gate_hist))
  (def prev_stage (read-history stage_hist))

  (def gate_on (gt gate_sig 0.5))
  (def gate_rising (* gate_on (lte prev_gate 0.5)))
  (def retrigger (max gate_rising trigger_sig))
  (def attack_stage 1.0)
  (def decay_stage 2.0)
  (def reset_stage 3.0)
  (def attack_done (gte prev_env 0.999))
  (def reset_done (lte prev_env 0.0001))

  (def stage_from_gate
    (gswitch gate_on
      (gswitch retrigger
        (gswitch (gt prev_env 0.0001) reset_stage attack_stage)
        prev_stage)
      0.0))

  (def stage
    (gswitch (eq stage_from_gate reset_stage)
      (gswitch reset_done attack_stage reset_stage)
      (gswitch attack_done
        (gswitch (eq stage_from_gate attack_stage) decay_stage stage_from_gate)
        stage_from_gate)))

  (def target
    (gswitch gate_on
      (gswitch (eq stage reset_stage)
        0.0
        (gswitch (eq stage attack_stage) 1.0 sustain))
      0.0))

  (def rate
    (gswitch gate_on
      (gswitch (eq stage reset_stage) reset_coeff decay_coeff)
      release_coeff))

  (def one_pole_level (+ prev_env (* rate (- target prev_env))))
  (def attack_level (+ prev_env (/ 1.0 attack_samples)))
  (def level_raw
    (gswitch (eq stage attack_stage)
      attack_level
      one_pole_level))
  (def level (clip level_raw 0 1))
  (write-history env level)
  (write-history gate_hist gate_sig)
  (write-history stage_hist stage)
  level)

; A finite-duration, power-curved ADSR. Both curve arguments are positive
; exponents: 1 is linear, values above 1 are convex, and values below 1 are
; concave. Separate attack/fall curves can model the concave attack and convex
; decay/release typical of analog RC envelopes. Sustain remains literal.
(defmacro adsrexp
  (gate_sig trigger_sig attack_ms decay_ms sustain release_ms attack_curve fall_curve)
  (make-history env)
  (make-history gate_hist)
  (make-history stage_hist)
  (make-history phase_hist)
  (make-history release_start_hist)

  (def sr samplerate)
  (def reset_samples (* 0.003 sr))
  (def reset_coeff (- 1.0 (exp (/ -6.907755 reset_samples))))
  ; The differentiable one-sample floor also matches the train-time analytic
  ; lowering exactly, including at a zero-millisecond duration.
  (def attack_samples (+ 1.0 (* attack_ms 0.001 sr)))
  (def decay_samples (+ 1.0 (* decay_ms 0.001 sr)))
  (def release_samples (+ 1.0 (* release_ms 0.001 sr)))
  (def attack_shape (max 0.01 attack_curve))
  (def fall_shape (max 0.01 fall_curve))
  ; Keep power bases strictly positive so learning either curve never
  ; encounters log(0), then normalize both shaped ranges to exact endpoints.
  (def curve_epsilon 0.000001)
  (def curve_domain (- 1.0 curve_epsilon))
  (def attack_curve_floor (pow curve_epsilon attack_shape))
  (def attack_curve_scale (/ 1.0 (- 1.0 attack_curve_floor)))
  (def fall_curve_floor (pow curve_epsilon fall_shape))
  (def fall_curve_scale (/ 1.0 (- 1.0 fall_curve_floor)))

  (def prev_env (read-history env))
  (def prev_gate (read-history gate_hist))
  (def prev_stage (read-history stage_hist))
  (def prev_phase (read-history phase_hist))
  (def prev_release_start (read-history release_start_hist))

  (def gate_on (gt gate_sig 0.5))
  (def gate_rising (* gate_on (lte prev_gate 0.5)))
  (def gate_falling (* (lte gate_sig 0.5) (gt prev_gate 0.5)))
  (def retrigger (max gate_rising trigger_sig))
  (def attack_stage 1.0)
  (def decay_stage 2.0)
  (def reset_stage 3.0)
  (def reset_done (lte prev_env 0.0001))
  ; A completed release also leaves phase at 1. Only treat that phase as an
  ; attack completion when this is a continuation of the previous attack,
  ; never on a fresh gate or trigger.
  (def attack_done
    (* (eq prev_stage attack_stage)
       (lte retrigger 0.5)
       (gte prev_phase 1.0)))

  (def stage_from_gate
    (gswitch gate_on
      (gswitch retrigger
        (gswitch (gt prev_env 0.0001) reset_stage attack_stage)
        prev_stage)
      0.0))
  (def stage
    (gswitch (eq stage_from_gate reset_stage)
      (gswitch reset_done attack_stage reset_stage)
      (gswitch (eq stage_from_gate attack_stage)
        (gswitch attack_done decay_stage attack_stage)
        stage_from_gate)))

  (def phase_start
    (gswitch (eq stage prev_stage) prev_phase 0.0))
  (def phase_step
    (gswitch (eq stage attack_stage)
      (/ 1.0 attack_samples)
      (gswitch (eq stage decay_stage)
        (/ 1.0 decay_samples)
        (gswitch gate_on 0.0 (/ 1.0 release_samples)))))
  (def phase
    (gswitch (eq stage reset_stage)
      0.0
      (clip (+ phase_start phase_step) 0.0 1.0)))

  (def release_start
    (gswitch gate_falling prev_env prev_release_start))
  (def attack_level
    (* (- (pow (+ curve_epsilon (* curve_domain phase)) attack_shape)
          attack_curve_floor)
       attack_curve_scale))
  (def remaining (- 1.0 phase))
  (def shaped_remaining
    (* (- (pow (+ curve_epsilon (* curve_domain remaining)) fall_shape)
          fall_curve_floor)
       fall_curve_scale))
  (def decay_level
    (+ sustain (* (- 1.0 sustain) shaped_remaining)))
  (def release_level (* release_start shaped_remaining))
  (def reset_level (+ prev_env (* reset_coeff (- 0.0 prev_env))))
  (def level_raw
    (gswitch gate_on
      (gswitch (eq stage reset_stage)
        reset_level
        (gswitch (eq stage attack_stage) attack_level decay_level))
      release_level))
  (def level (clip level_raw 0.0 1.0))

  (write-history env level)
  (write-history gate_hist gate_sig)
  (write-history stage_hist stage)
  (write-history phase_hist phase)
  (write-history release_start_hist release_start)
  level)
"#;

pub(in crate::lisp_host) fn instrument_preamble(sample_rate: u32) -> String {
    let _ = sample_rate;
    INSTRUMENT_PREAMBLE.to_string()
}

pub(in crate::lisp_host) fn effect_preamble(sample_rate: u32) -> String {
    instrument_preamble(sample_rate)
}

/// Produce the exact instrument source handed to DGenLisp after factory
/// defmacro imports, the instrument preamble, and param hoisting are applied.
/// Long-lived external-tool workflows must snapshot this form rather than the
/// raw editor document so their evaluator sees the same language as hot-swap.
pub fn effective_instrument_source(source: &str, sample_rate: u32) -> Result<String, String> {
    let effective = effective_dgen_source(DGenCompileKind::Instrument, source, sample_rate)?;
    Ok(finalize_effective_dgen_source(&effective))
}

pub fn compile_instrument(source: &str, sample_rate: u32) -> Result<String, String> {
    compile_instrument_with_asset_base(source, sample_rate, None)
}

pub fn compile_instrument_with_asset_base(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
) -> Result<String, String> {
    let dir = output_dir();
    let seq = COMPILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    // Include the pid: concurrent test processes share output_dir() and each
    // starts COMPILE_COUNTER at 0, so a bare counter collides across them.
    let dylib_name = format!("instrument_{}_{}", std::process::id(), seq);
    let effective_source = effective_dgen_source(DGenCompileKind::Instrument, source, sample_rate)?;
    compile_effective_dgen_source_to_dir(
        DGenCompileKind::Instrument,
        &effective_source,
        sample_rate,
        asset_base,
        &dir,
        &dylib_name,
    )
}

pub(in crate::lisp_host) fn log_dgenlisp_compile_failure(kind: &str, src_path: &Path, error: &str, source: &str) {
    eprintln!(
        "[dgenlisp compile failed] kind={kind} path={}\nerror:\n{error}\nsource:\n{source}\n[/dgenlisp compile failed]",
        src_path.display()
    );
}

pub(in crate::lisp_host) fn log_dgenlisp_compile_manifest(kind: &str, src_path: &Path, manifest: &str) {
    /*
    eprintln!(
        "[dgenlisp compile manifest] kind={kind} path={}\nmanifest:\n{manifest}\n[/dgenlisp compile manifest]",
        src_path.display()
    );
    */
}

pub fn compile_and_load_instrument(
    source: &str,
    sample_rate: u32,
) -> Result<CompileResult, String> {
    compile_and_load_instrument_with_asset_base(source, sample_rate, None)
}

pub fn compile_and_load_instrument_with_asset_base(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
) -> Result<CompileResult, String> {
    compile_and_load_instrument_with_origin(
        source,
        sample_rate,
        asset_base,
        DGenSourceOrigin::Custom,
    )
}

pub fn compile_and_load_instrument_with_origin(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
    origin: DGenSourceOrigin,
) -> Result<CompileResult, String> {
    let mut result = dylib_cache::global_cache_manager().acquire(
        DGenCompileKind::Instrument,
        origin,
        source,
        sample_rate,
        asset_base,
    )?;
    result.manifest.asset_base = asset_base.map(|base| {
        eseqlisp::widget_render::patcher::register_asset_source_root(base)
    });
    Ok(result)
}

pub fn compile_and_load_instrument_uncached_with_asset_base(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
) -> Result<CompileResult, String> {
    let json = compile_instrument_with_asset_base(source, sample_rate, asset_base)?;
    let mut manifest = parse_manifest(&json)?;
    manifest.asset_base = asset_base.map(|base| {
        eseqlisp::widget_render::patcher::register_asset_source_root(base)
    });
    let lib = load_dylib_prewarmed(&manifest)?;
    Ok(CompileResult {
        manifest,
        lib,
        lease: None,
    })
}

pub fn render_instrument_source_for_test(
    source: &str,
    asset_base: Option<&Path>,
    options: &InstrumentRenderOptions,
) -> Result<InstrumentRenderReport, String> {
    let result =
        compile_and_load_instrument_with_asset_base(source, options.sample_rate, asset_base)?;
    render_loaded_instrument_for_test(&result.manifest, &result.lib, options)
}

pub fn render_loaded_instrument_for_test(
    manifest: &DGenManifest,
    lib: &LoadedDGenLib,
    options: &InstrumentRenderOptions,
) -> Result<InstrumentRenderReport, String> {
    if options.block_size == 0 {
        return Err("block_size must be greater than zero".to_string());
    }
    if options.frames == 0 {
        return Err("frames must be greater than zero".to_string());
    }

    let total_slots = manifest.total_memory_slots;
    let mut memory = vec![0.0f32; total_slots + DGEN_STATE_REDZONE_SLOTS];
    let slot_id = options.voice_index;
    let init_msg = build_init_message_for_voice(slot_id, manifest, options.voice_index);
    let entry_count = init_msg.get(9).copied().unwrap_or(0.0) as usize;
    for i in 0..entry_count {
        let idx = init_msg[10 + i * 2] as usize;
        let value = init_msg[10 + i * 2 + 1];
        if idx < total_slots {
            memory[idx] = value;
        }
    }

    let apply_param = |memory: &mut [f32], name: &str, value: f32| -> Result<(), String> {
        let param = manifest
            .params
            .iter()
            .find(|param| param.name == name)
            .ok_or_else(|| format!("unknown instrument parameter '{name}'"))?;
        if param.cell_id >= total_slots {
            return Err(format!(
                "parameter '{}' cell {} is outside memory size {}",
                param.name, param.cell_id, total_slots
            ));
        }
        for lane in 0..param.cell_span {
            let idx = param.cell_id + lane;
            if idx < total_slots {
                memory[idx] = value;
            }
        }
        Ok(())
    };

    for (name, value) in &options.param_overrides {
        apply_param(&mut memory, name, *value)?;
    }

    let mut param_events = options.param_events.clone();
    param_events.sort_by_key(|event| event.frame);
    let mut next_param_event = 0usize;

    let pitch_hz = 440.0 * 2f32.powf((options.midi_note - 69.0) / 12.0);
    let n_inputs = manifest.n_inputs.max(4);
    let n_outputs = manifest.n_outputs.max(1);
    let input_routes: Vec<_> = manifest.inputs.iter().filter_map(|input| {
        manifest.host_signal_output_for_input(input).map(|output| (input.channel, output))
    }).collect();
    let mut rendered = Vec::with_capacity(options.frames);
    let mut frames_done = 0usize;

    while frames_done < options.frames {
        while next_param_event < param_events.len()
            && param_events[next_param_event].frame <= frames_done
        {
            let event = &param_events[next_param_event];
            apply_param(&mut memory, &event.name, event.value)?;
            next_param_event += 1;
        }

        let next_event_frame = param_events
            .get(next_param_event)
            .map(|event| event.frame)
            .unwrap_or(options.frames)
            .max(frames_done);
        let block_limit = options.block_size.min(options.frames - frames_done);
        let block = block_limit.min((next_event_frame - frames_done).max(1));
        let trigger_value = if frames_done == 0 { 1.0 } else { 0.0 };

        let mut input_buffers = vec![vec![0.0f32; block]; n_inputs];
        for &(channel, output) in &input_routes {
            let buffer = input_buffers.get_mut(channel)
                .ok_or_else(|| format!("Manifest input channel {channel} exceeds {n_inputs} inputs"))?;
            use crate::effects::gatepitch as gp;
            match output {
                output if output == gp::PARAM_GATE as usize => {
                    for (frame, value) in buffer.iter_mut().enumerate() {
                        *value = if frames_done + frame < options.gate_frames { 1.0 } else { 0.0 };
                    }
                }
                output if output == gp::PARAM_PITCH as usize => buffer.fill(pitch_hz),
                output if output == gp::PARAM_VELOCITY as usize => buffer.fill(options.velocity),
                output if output == gp::PARAM_TRIGGER as usize || output == gp::OUTPUT_NOTE_ON => {
                    buffer[0] = trigger_value;
                }
                // A single isolated note has no legato/pressure/transport
                // input unless the caller explicitly supplies an override.
                _ => {}
            }
        }
        for &(channel, value) in &options.input_overrides {
            if let Some(buffer) = input_buffers.get_mut(channel) {
                buffer.fill(value);
            }
        }
        let input_ptrs: Vec<*const f32> = input_buffers
            .iter()
            .map(|buffer| buffer.as_ptr())
            .collect();

        let mut output_buffers = vec![vec![0.0f32; block]; n_outputs];
        let output_ptrs: Vec<*mut f32> = output_buffers
            .iter_mut()
            .map(|buffer| buffer.as_mut_ptr())
            .collect();

        let context = dgen_process_context_v1(options.sample_rate.max(1) as f32);
        unsafe {
            (lib.process_fn)(
                input_ptrs.as_ptr(),
                output_ptrs.as_ptr(),
                block as u32,
                memory.as_mut_ptr() as *mut c_void,
                &context,
                dgen_host_services_v1(),
            );
        }
        rendered.extend_from_slice(&output_buffers[0]);
        frames_done += block;
    }

    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f64;
    let mut sum_abs = 0.0f64;
    let mut nonzero_frames = 0usize;
    let mut first_nonzero_frame = None;
    let mut non_finite_samples = 0usize;
    let mut first_non_finite_frame = None;
    for (idx, sample) in rendered.iter().enumerate() {
        if !sample.is_finite() {
            non_finite_samples += 1;
            if first_non_finite_frame.is_none() {
                first_non_finite_frame = Some(idx);
            }
            continue;
        }
        let abs = sample.abs();
        peak = peak.max(abs);
        sum_sq += (*sample as f64) * (*sample as f64);
        sum_abs += abs as f64;
        if abs > 1.0e-7 {
            nonzero_frames += 1;
            if first_nonzero_frame.is_none() {
                first_nonzero_frame = Some(idx);
            }
        }
    }
    let frames = rendered.len().max(1);
    let rms = (sum_sq / frames as f64).sqrt() as f32;
    let mean_abs = (sum_abs / frames as f64) as f32;
    let mut non_finite_state_slots = 0usize;
    let mut first_non_finite_state_slot = None;
    for idx in 0..total_slots {
        if !memory[idx].is_finite() {
            non_finite_state_slots += 1;
            if first_non_finite_state_slot.is_none() {
                first_non_finite_state_slot = Some(idx);
            }
        }
    }

    Ok(InstrumentRenderReport {
        frames: rendered.len(),
        peak,
        rms,
        mean_abs,
        nonzero_frames,
        first_nonzero_frame,
        non_finite_samples,
        first_non_finite_frame,
        non_finite_state_slots,
        first_non_finite_state_slot,
        first_samples: rendered.into_iter().take(32).collect(),
    })
}

pub fn render_effect_source_for_test(
    source: &str,
    options: &EffectRenderOptions,
) -> Result<EffectRenderReport, String> {
    let result = compile_and_load(source, options.sample_rate)?;
    render_loaded_effect_for_test(&result.manifest, &result.lib, options)
}

pub fn render_loaded_effect_for_test(
    manifest: &DGenManifest,
    lib: &LoadedDGenLib,
    options: &EffectRenderOptions,
) -> Result<EffectRenderReport, String> {
    render_loaded_effect_for_test_with_host_services(
        manifest,
        lib,
        options,
        dgen_host_services_v1(),
    )
}

pub(in crate::lisp_host) fn render_loaded_effect_for_test_with_host_services(
    manifest: &DGenManifest,
    lib: &LoadedDGenLib,
    options: &EffectRenderOptions,
    host_services: *const DGenHostServicesV1,
) -> Result<EffectRenderReport, String> {
    if host_services.is_null() {
        return Err("DGen host-services table must not be NULL".to_string());
    }
    if options.block_size == 0 {
        return Err("block_size must be greater than zero".to_string());
    }
    if options.frames == 0 {
        return Err("frames must be greater than zero".to_string());
    }
    if manifest.n_inputs < 2 || manifest.n_outputs < 2 {
        return Err(format!(
            "effect probe requires at least two inputs and outputs, got {} input(s) and {} output(s)",
            manifest.n_inputs, manifest.n_outputs
        ));
    }

    let total_slots = manifest.total_memory_slots;
    let mut memory = vec![0.0f32; total_slots + DGEN_STATE_REDZONE_SLOTS];
    let init_msg = build_init_message(0, manifest, None);
    let entry_count = init_msg.get(9).copied().unwrap_or(0.0) as usize;
    for i in 0..entry_count {
        let idx = init_msg[10 + i * 2] as usize;
        let value = init_msg[10 + i * 2 + 1];
        if idx < total_slots {
            memory[idx] = value;
        }
    }

    let apply_param = |memory: &mut [f32], name: &str, value: f32| -> Result<(), String> {
        if let Some(param) = manifest.params.iter().find(|param| param.name == name) {
            if param.cell_id >= total_slots {
                return Err(format!(
                    "parameter '{}' cell {} is outside memory size {}",
                    param.name, param.cell_id, total_slots
                ));
            }
            for lane in 0..param.cell_span {
                let idx = param.cell_id + lane;
                if idx < total_slots {
                    memory[idx] = value;
                }
            }
            return Ok(());
        }

        let Some(cell_id) = host_mod_descriptor_param_cell(manifest, name) else {
            return Err(format!("unknown effect parameter '{name}'"));
        };
        if cell_id >= total_slots {
            return Err(format!(
                "parameter '{name}' cell {cell_id} is outside memory size {total_slots}"
            ));
        }
        memory[cell_id] = value;
        Ok(())
    };

    for (name, value) in &options.param_overrides {
        apply_param(&mut memory, name, *value)?;
    }

    let mut param_events = options.param_events.clone();
    param_events.sort_by_key(|event| event.frame);
    let mut next_param_event = 0usize;

    for (name, values) in &options.tensor_overrides {
        let tensor = manifest
            .tensors
            .iter()
            .find(|tensor| tensor.name == *name)
            .ok_or_else(|| format!("unknown effect tensor '{name}'"))?;
        let expected_len = tensor.shape.iter().product::<usize>();
        if values.len() != expected_len {
            return Err(format!(
                "effect tensor '{}' override has {} values, expected {} for shape {:?}",
                name,
                values.len(),
                expected_len,
                tensor.shape,
            ));
        }
        let end = tensor
            .cell_offset
            .checked_add(expected_len)
            .ok_or_else(|| format!("effect tensor '{name}' memory range overflow"))?;
        let destination = memory.get_mut(tensor.cell_offset..end).ok_or_else(|| {
            format!(
                "effect tensor '{}' cells {}..{} are outside memory size {}",
                name,
                tensor.cell_offset,
                end,
                total_slots,
            )
        })?;
        destination.copy_from_slice(values);
    }

    let n_inputs = manifest.n_inputs.max(2);
    let n_outputs = manifest.n_outputs.max(2);
    let mut rendered = Vec::with_capacity(options.frames * 2);
    let mut input_reference = Vec::with_capacity(options.frames * 2);
    let mut frames_done = 0usize;

    while frames_done < options.frames {
        while next_param_event < param_events.len()
            && param_events[next_param_event].frame <= frames_done
        {
            let event = &param_events[next_param_event];
            apply_param(&mut memory, &event.name, event.value)?;
            next_param_event += 1;
        }
        let next_event_frame = param_events
            .get(next_param_event)
            .map(|event| event.frame)
            .unwrap_or(options.frames)
            .max(frames_done);
        let block_limit = options.block_size.min(options.frames - frames_done);
        let block = block_limit.min((next_event_frame - frames_done).max(1));
        let mut input_buffers = vec![vec![0.0f32; block]; n_inputs];
        for frame in 0..block {
            let t = (frames_done + frame) as f32 / options.sample_rate.max(1) as f32;
            let impulse = if frames_done + frame == 0 { 0.45 } else { 0.0 };
            let burst_env = (1.0 - (t * 4.0)).max(0.0);
            let left = impulse
                + 0.18
                    * burst_env
                    * ((2.0 * std::f32::consts::PI * 220.0 * t).sin()
                        + 0.5 * (2.0 * std::f32::consts::PI * 997.0 * t).sin());
            let right = 0.12
                * burst_env
                * ((2.0 * std::f32::consts::PI * 330.0 * t).sin()
                    + 0.5 * (2.0 * std::f32::consts::PI * 1409.0 * t).sin());
            input_buffers[0][frame] = left;
            input_buffers[1][frame] = right;
        }
        for &(channel, value) in &options.input_overrides {
            if let Some(buffer) = input_buffers.get_mut(channel) {
                buffer.fill(value);
            }
        }
        let mut tone_filled: Vec<usize> = Vec::new();
        for &(channel, freq, amp) in &options.input_tones {
            let Some(buffer) = input_buffers.get_mut(channel) else {
                continue;
            };
            let replace = !tone_filled.contains(&channel);
            for (frame, sample) in buffer.iter_mut().enumerate() {
                let t = (frames_done + frame) as f32 / options.sample_rate.max(1) as f32;
                let tone = amp * (2.0 * std::f32::consts::PI * freq * t).sin();
                if replace {
                    *sample = tone;
                } else {
                    *sample += tone;
                }
            }
            tone_filled.push(channel);
        }
        for frame in 0..block {
            input_reference.push(input_buffers[0][frame]);
            input_reference.push(input_buffers[1][frame]);
        }
        let input_ptrs: Vec<*const f32> = input_buffers
            .iter()
            .map(|buffer| buffer.as_ptr())
            .collect();

        let mut output_buffers = vec![vec![0.0f32; block]; n_outputs];
        let output_ptrs: Vec<*mut f32> = output_buffers
            .iter_mut()
            .map(|buffer| buffer.as_mut_ptr())
            .collect();

        let context = dgen_process_context_v1(options.sample_rate.max(1) as f32);
        unsafe {
            (lib.process_fn)(
                input_ptrs.as_ptr(),
                output_ptrs.as_ptr(),
                block as u32,
                memory.as_mut_ptr() as *mut c_void,
                &context,
                host_services,
            );
        }
        for frame in 0..block {
            rendered.push(output_buffers[0][frame]);
            rendered.push(output_buffers[1][frame]);
        }
        frames_done += block;
    }

    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f64;
    let mut sum_abs = 0.0f64;
    let mut diff_sq = 0.0f64;
    let mut nonzero_frames = 0usize;
    let mut first_nonzero_frame = None;
    for (idx, sample) in rendered.iter().enumerate() {
        let abs = sample.abs();
        peak = peak.max(abs);
        sum_sq += (*sample as f64) * (*sample as f64);
        sum_abs += abs as f64;
        let input = input_reference.get(idx).copied().unwrap_or(0.0);
        let diff = *sample - input;
        diff_sq += (diff as f64) * (diff as f64);
        if abs > 1.0e-7 {
            nonzero_frames += 1;
            if first_nonzero_frame.is_none() {
                first_nonzero_frame = Some(idx / 2);
            }
        }
    }
    let samples = rendered.len().max(1);
    let rms = (sum_sq / samples as f64).sqrt() as f32;
    let mean_abs = (sum_abs / samples as f64) as f32;
    let diff_rms = (diff_sq / samples as f64).sqrt() as f32;
    let mut left_sq = 0.0f64;
    let mut right_sq = 0.0f64;
    let mut stereo_frames = 0usize;
    for frame in rendered.chunks_exact(2) {
        left_sq += (frame[0] as f64) * (frame[0] as f64);
        right_sq += (frame[1] as f64) * (frame[1] as f64);
        stereo_frames += 1;
    }
    let stereo_frames = stereo_frames.max(1) as f64;
    let left_rms = (left_sq / stereo_frames).sqrt() as f32;
    let right_rms = (right_sq / stereo_frames).sqrt() as f32;

    Ok(EffectRenderReport {
        frames: options.frames,
        peak,
        rms,
        left_rms,
        right_rms,
        mean_abs,
        diff_rms,
        nonzero_frames,
        first_nonzero_frame,
        first_samples: rendered.iter().copied().take(32).collect(),
        samples: rendered,
    })
}

pub(in crate::lisp_host) fn host_mod_descriptor_param_cell(manifest: &DGenManifest, name: &str) -> Option<usize> {
    for dest in &manifest.mod_destinations {
        if name == format!("__dgen_mod_active__{}", dest.name) {
            return Some(dest.active_cell_id);
        }
        for lane in &dest.depth_lanes {
            if name == format!("mod {} slot {} amt", dest.name, lane.slot) {
                return Some(lane.depth_cell_id);
            }
        }
    }
    None
}

// ── Instrument editor flow ──

pub const INSTRUMENT_TEMPLATE: &str = r#"; DGenLisp instrument
;
; Params:  (param name @default 1.0 @min 0 @max 10)
; Modulatable: add @mod true @mod-mode additive
;   then use (mod name) to read the modulated value
; Envelope: (adsr gate trigger attack_ms decay_ms sustain release_ms)
; Curved envelope:
;   (adsrexp gate trigger attack_ms decay_ms sustain release_ms attack_curve fall_curve)
;   curves are positive exponents; 1 is linear, >1 convex, and <1 concave.
; Oscillators: (phasor freq_hz), (sin expr), (noise)
; Math: +, -, *, /, sin, cos, tan, atan, atan2, tanh, clamp, min, max
; Constants: twopi, samplerate

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

; -- Parameters --
(param attack  @default 5    @min 0   @max 1000 @unit ms)
(param decay   @default 120  @min 1   @max 2000 @unit ms)
(param sustain @default 0.8  @min 0   @max 1)
(param release @default 180  @min 1   @max 5000 @unit ms)
(param gain    @default 0.5  @min 0   @max 1    @mod true @mod-mode additive)

; -- Envelope --
(def env (adsr gate trigger attack decay sustain release))

; -- Oscillator --
(def phase (phasor pitch))

; -- Output --
(out (* phase env velocity (mod gain)) 1 @name audio)
"#;

pub struct InstrumentEditResult {
    pub manifest: DGenManifest,
    pub lib: LoadedDGenLib,
    pub lease: Option<DylibLease>,
    pub source: String,
    pub params: Vec<DGenParam>,
    pub name: String,
}
