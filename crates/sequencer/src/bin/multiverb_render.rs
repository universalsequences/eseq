//! Multiverb offline render harness (spec: docs/reverb-modes-spec.md).
//!
//! Renders a test signal (or an input wav) through the Multiverb builtin and
//! writes a stereo f32 wav plus EDC decay-time readings to stderr — the
//! ear-tuning + metrics loop for the reverb modes.
//!
//! multiverb_render --wav out.wav [--signal impulse|clicks|burst] [--in file.wav]
//!   [--sr 44100] [--seconds 6.0] [--mode 0] [--decay 0.55] [--size 0.5]
//!   [--predelay 0] [--damp 0.35] [--bass 0.5] [--diffusion 0.7] [--rate 0.7]
//!   [--depth 0.15] [--shape 0.0] [--era 0.15] [--width 1.0] [--mix 1.0]
//!   [--mod1 0.0] ... [--mod4 0.0]
//!   [--mod-decay-1 0.0] ... [--mod-mix-4 0.0]

use sequencer::multiverb::{
    multiverb_vtable, MULTIVERB_PARAM_BASS, MULTIVERB_PARAM_DAMP, MULTIVERB_PARAM_DECAY,
    MULTIVERB_PARAM_DIFFUSION, MULTIVERB_PARAM_ERA, MULTIVERB_PARAM_MIX, MULTIVERB_PARAM_MODE,
    MULTIVERB_PARAM_MOD_DECAY_DEPTH_1, MULTIVERB_PARAM_MOD_DEPTH,
    MULTIVERB_PARAM_MOD_DEPTH_DEPTH_1, MULTIVERB_PARAM_MOD_MIX_DEPTH_1, MULTIVERB_PARAM_MOD_RATE,
    MULTIVERB_PARAM_MOD_SHAPE, MULTIVERB_PARAM_MOD_SIZE_DEPTH_1, MULTIVERB_PARAM_PREDELAY_MS,
    MULTIVERB_PARAM_SIZE, MULTIVERB_PARAM_WIDTH, MULTIVERB_STATE_SIZE,
};

fn modulation_target_flag(flag: &str) -> Option<u64> {
    for (prefix, base) in [
        ("--mod-decay-", MULTIVERB_PARAM_MOD_DECAY_DEPTH_1),
        ("--mod-size-", MULTIVERB_PARAM_MOD_SIZE_DEPTH_1),
        ("--mod-depth-", MULTIVERB_PARAM_MOD_DEPTH_DEPTH_1),
        ("--mod-mix-", MULTIVERB_PARAM_MOD_MIX_DEPTH_1),
    ] {
        let Some(suffix) = flag.strip_prefix(prefix) else {
            continue;
        };
        let slot = suffix.parse::<u64>().ok()?;
        if (1..=4).contains(&slot) {
            return Some(base + slot - 1);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modulation_target_flags_cover_all_four_destinations_and_slots() {
        for (prefix, base) in [
            ("--mod-decay-", MULTIVERB_PARAM_MOD_DECAY_DEPTH_1),
            ("--mod-size-", MULTIVERB_PARAM_MOD_SIZE_DEPTH_1),
            ("--mod-depth-", MULTIVERB_PARAM_MOD_DEPTH_DEPTH_1),
            ("--mod-mix-", MULTIVERB_PARAM_MOD_MIX_DEPTH_1),
        ] {
            for slot in 1..=4 {
                assert_eq!(
                    modulation_target_flag(&format!("{prefix}{slot}")),
                    Some(base + slot - 1)
                );
            }
        }
        assert_eq!(modulation_target_flag("--mod-decay-0"), None);
        assert_eq!(modulation_target_flag("--mod-mix-5"), None);
        assert_eq!(modulation_target_flag("--depth"), None);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut sr = 44_100usize;
    let mut seconds = 6.0f32;
    let mut wav_path: Option<String> = None;
    let mut in_path: Option<String> = None;
    let mut signal = "impulse".to_string();
    let mut mod_values = [0.0f32; 4];
    // (param slot, value) pairs applied after init.
    let mut params: Vec<(u64, f32)> = vec![(MULTIVERB_PARAM_MIX, 1.0)];

    let flag_map: &[(&str, u64)] = &[
        ("--mode", MULTIVERB_PARAM_MODE),
        ("--decay", MULTIVERB_PARAM_DECAY),
        ("--size", MULTIVERB_PARAM_SIZE),
        ("--predelay", MULTIVERB_PARAM_PREDELAY_MS),
        ("--damp", MULTIVERB_PARAM_DAMP),
        ("--bass", MULTIVERB_PARAM_BASS),
        ("--diffusion", MULTIVERB_PARAM_DIFFUSION),
        ("--rate", MULTIVERB_PARAM_MOD_RATE),
        ("--depth", MULTIVERB_PARAM_MOD_DEPTH),
        ("--shape", MULTIVERB_PARAM_MOD_SHAPE),
        ("--era", MULTIVERB_PARAM_ERA),
        ("--width", MULTIVERB_PARAM_WIDTH),
        ("--mix", MULTIVERB_PARAM_MIX),
    ];

    let mut i = 1;
    while i < args.len() {
        let need = |i: usize| -> String {
            args.get(i + 1)
                .unwrap_or_else(|| panic!("{} needs a value", args[i]))
                .clone()
        };
        match args[i].as_str() {
            "--sr" => {
                sr = need(i).parse().expect("--sr");
                i += 1;
            }
            "--seconds" => {
                seconds = need(i).parse().expect("--seconds");
                i += 1;
            }
            "--wav" => {
                wav_path = Some(need(i));
                i += 1;
            }
            "--in" => {
                in_path = Some(need(i));
                i += 1;
            }
            "--signal" => {
                signal = need(i);
                i += 1;
            }
            "--mod1" | "--mod2" | "--mod3" | "--mod4" => {
                let slot = args[i]
                    .strip_prefix("--mod")
                    .and_then(|value| value.parse::<usize>().ok())
                    .expect("mod input flag")
                    - 1;
                mod_values[slot] = need(i)
                    .parse::<f32>()
                    .expect("--modN needs a numeric value")
                    .clamp(0.0, 1.0);
                i += 1;
            }
            flag => {
                if let Some(&(_, slot)) = flag_map.iter().find(|(name, _)| *name == flag) {
                    params.push((
                        slot,
                        need(i)
                            .parse()
                            .unwrap_or_else(|_| panic!("{flag} needs a numeric value")),
                    ));
                    i += 1;
                } else if let Some(slot) = modulation_target_flag(flag) {
                    params.push((
                        slot,
                        need(i)
                            .parse()
                            .unwrap_or_else(|_| panic!("{flag} needs a numeric value")),
                    ));
                    i += 1;
                } else {
                    panic!("unknown flag {flag}");
                }
            }
        }
        i += 1;
    }
    let wav_path = wav_path.expect("--wav <out.wav> is required");

    // Build the input signal.
    let (mut in_l, mut in_r) = if let Some(path) = in_path {
        let mut reader = hound::WavReader::open(&path).expect("open --in wav");
        let spec = reader.spec();
        sr = spec.sample_rate as usize;
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| s.unwrap() as f32 * scale)
                    .collect()
            }
        };
        let ch = spec.channels as usize;
        let frames = samples.len() / ch;
        let mut l = Vec::with_capacity(frames);
        let mut r = Vec::with_capacity(frames);
        for f in 0..frames {
            l.push(samples[f * ch]);
            r.push(samples[f * ch + ch.min(2) - 1]);
        }
        // Leave room for the tail.
        let tail = (seconds * sr as f32) as usize;
        l.extend(std::iter::repeat(0.0).take(tail));
        r.extend(std::iter::repeat(0.0).take(tail));
        (l, r)
    } else {
        let total = (seconds * sr as f32) as usize;
        let mut l = vec![0.0f32; total];
        match signal.as_str() {
            "impulse" => l[0] = 1.0,
            "clicks" => {
                let mut pos = 0;
                while pos < total {
                    l[pos] = 1.0;
                    pos += sr; // one click per second
                }
            }
            "burst" => {
                // 50 ms noise burst — echo-density / bloom inspection.
                let mut rng = 0x9e3779b9u32;
                for x in l.iter_mut().take(sr / 20) {
                    rng ^= rng << 13;
                    rng ^= rng >> 17;
                    rng ^= rng << 5;
                    *x = (rng as f32 / u32::MAX as f32 - 0.5) * 0.5;
                }
            }
            other => panic!("unknown --signal {other} (impulse|clicks|burst)"),
        }
        let r = l.clone();
        (l, r)
    };

    // Init + set params.
    let mut state = vec![0.0f32; MULTIVERB_STATE_SIZE];
    let vt = multiverb_vtable();
    unsafe {
        (vt.init.unwrap())(state.as_mut_ptr().cast(), sr as i32, 512, std::ptr::null());
    }
    for &(slot, value) in &params {
        state[slot as usize] = value;
    }

    // Render.
    let total = in_l.len();
    let mut out_l = vec![0.0f32; total];
    let mut out_r = vec![0.0f32; total];
    let block = 256;
    let mut done = 0;
    while done < total {
        let n = block.min(total - done);
        let mut mod_buffers: [Vec<f32>; 4] = std::array::from_fn(|slot| vec![mod_values[slot]; n]);
        let [mod_1, mod_2, mod_3, mod_4] = &mut mod_buffers;
        let inputs = [
            in_l[done..].as_mut_ptr(),
            in_r[done..].as_mut_ptr(),
            mod_1.as_mut_ptr(),
            mod_2.as_mut_ptr(),
            mod_3.as_mut_ptr(),
            mod_4.as_mut_ptr(),
        ];
        let outputs = [out_l[done..].as_mut_ptr(), out_r[done..].as_mut_ptr()];
        unsafe {
            (vt.process.unwrap())(
                inputs.as_ptr(),
                outputs.as_ptr(),
                n as i32,
                state.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            );
        }
        done += n;
    }

    // EDC decay readings (impulse-ish signals only, but harmless otherwise):
    // backwards-integrated energy, report time to fall 10/20/30/60 dB.
    let energy: Vec<f64> = {
        let mut acc = 0.0f64;
        let mut rev: Vec<f64> = out_l
            .iter()
            .rev()
            .map(|x| {
                acc += (*x as f64) * (*x as f64);
                acc
            })
            .collect();
        rev.reverse();
        rev
    };
    if energy[0] > 0.0 {
        for db in [10.0f64, 20.0, 30.0, 60.0] {
            let target = energy[0] * 10f64.powf(-db / 10.0);
            let t = energy
                .iter()
                .position(|e| *e <= target)
                .map(|n| n as f64 / sr as f64);
            match t {
                Some(t) => eprintln!(
                    "EDC -{db:.0} dB: {t:.3} s (RT60 est {:.2} s)",
                    t * 60.0 / db
                ),
                None => eprintln!("EDC -{db:.0} dB: not reached in render"),
            }
        }
    }

    // Write stereo wav.
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: sr as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&wav_path, spec).expect("create wav");
    for f in 0..total {
        w.write_sample(out_l[f]).unwrap();
        w.write_sample(out_r[f]).unwrap();
    }
    w.finalize().unwrap();
    eprintln!("wrote {wav_path} ({total} frames @ {sr} Hz)");
}
