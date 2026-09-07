# Spectral hop scheduling investigation

## Resolution — DGenLisp v0.1.9

Fixed upstream in `d315fa5a7dc352c377ed6ff91b87c59680e89b2f`, published as
[DGenLisp v0.1.9](https://github.com/universalsequences/dgen-audio/releases/tag/dgenlisp-v0.1.9),
and fetched here through the updated macOS stanza in `content/dgenlisp.lock`.
Archive SHA-256: `035790ab0fe6e096e49a86b428b67c9324cdd7d4526165db751520810989857a`.

The compiler now separates scalar emission regions regardless of direct tensor
inputs, preserves shared hop lifetimes across independent static setup, and
honors write-after-write scheduling hazards. No host-block slicing or enlarged
hop workaround was introduced.

26 focused Swift tests passed in the clean release checkout, including a new
manual/accelerated FFT regression checking both independent stereo waveforms
and analytic scalar smoother cadence. Both downstream tests below pass with
the fetched compiler. The shared-history regression is enabled on macOS; Linux
remains on its old compiler pin, with publication tracked as `eseq-wci0`.

The effect is installed at `.local/effects/spectral-tamer/dsp.lisp`. Validation
passed at 44.1/48/96 kHz and host blocks 64/128/256/512, including active reduction,
zero-amount identity, stereo isolation, wet+delta reconstruction, silent sidechain,
and freeze/thaw. At 48 kHz: zero-amount error <1e-6, block-size difference exactly
zero, and wet+delta error <9e-8. It is not claimed to be sample-identical to a
commercial reference or ear-approved. Latency is 511 samples; authored custom
latency reporting remains a host limitation (`eseq-b72f`).

Tracking: implementation `eseq-ehcw`, compiler fix `eseq-yxau` / upstream `dgen-ka5`.

## Original investigation — v0.1.8 (`a2d037c`), 2026-09-06

A plain stereo Hann-windowed FFT/IFFT round trip is correct across host block
sizes 64, 128, 256 and 512, for FFT lengths 512, 1024 and 2048 with fourfold
overlap. All 12 combinations reconstruct their independent analytic channel
inputs within 1e-7, after latency/startup, and outputs are identical across host
block sizes.

Adding a shared audio-rate one-pole gain smoother *after* stereo overlap-add
reproduces a compiler scheduling failure. No detector, spectral envelope,
sidechain, or nonlinear audio processing is needed. This makes the earlier
blanket description of a multi-hop stereo FFT failure too broad: composition
with scalar feedback is sufficient to trigger it, while FFT identity alone
passes.

## Durable reproduction

Fixture:
`crates/sequencer/tests/fixtures/effects/stereo-stft-shared-output-history.lisp`

Tests:
`crates/sequencer/tests/spectral_hop_scheduling.rs`

Passing control:

```sh
cargo nextest run -p sequencer --test spectral_hop_scheduling \
  -E 'test(=stereo_hann_round_trip_is_independent_of_host_block_size)' \
  --no-capture
```

Previously failing compiler regression (now passing on macOS):

```sh
cargo nextest run -p sequencer --test spectral_hop_scheduling \
  -E 'test(=stereo_stft_shared_output_history_is_independent_of_host_block_size)' \
  --no-capture
```

Before the fix, the regression was explicitly ignored rather than weakened to
accept corruption. On v0.1.8 it failed in about one second with these max absolute
sample differences versus a 64-frame render:

| Host block | Hop | Error |
|---|---|---|
| 64 | 128 | 0 (reference) |
| 128 | 128 | 0 |
| 256 | 128 | 0.23416674 |
| 512 | 128 | 0.28737748 |

The probes use independent 997 Hz / 1409 Hz tones on L/R. The standalone
reproducer also runs outside the application audio graph through the generated
ABI, so project/UI state is not required to trigger the defect.

## Generated-code evidence

For the equivalent reduced fixture, emitted C separates the IFFT work from
later frame consumption into whole-host-block loops. A single tensor buffer
contains the last IFFT result when the later loop starts. That loop then copies
it to per-hop slots, too late to retain the intermediate hop results.

The generated C also puts the scalar smoother history update inside a
512-element tensor loop. It should advance once per audio sample, not once per
tensor element. Thus block-size invariance alone will not be enough to validate
the eventual fix: scalar recurrence cadence also needs an analytic assertion.

The corresponding local investigation artifacts are under
`/tmp/spectral-codegen/` and `/tmp/spectral-diagnose/`; these are disposable.
The durable fixture above can regenerate the compiler output with the compiler's
`--debug` flag.

This isolated a compiler rate/lifetime/scheduling defect. The initial investigation
did not alter the compiler or host chunking. Increasing the hop to match the host
block would mask the failure and change the algorithm, not fix it. The subsequent
upstream fix described above addresses scheduling and scalar cadence instead.

## Initial blocked effect attempt

The draft magnitude-driven spectral effect compiled and reconstructed correctly
at zero Amount for host blocks 64/128, but failed at 256/512. Removing its output
smoothing alone did not fix the complete draft, so the minimal trigger is not
the full scope of the scheduling problem.

The draft was removed from the loadable `.local/effects/` directory and retained
at `/tmp/spectral-tamer-blocked/spectral-tamer/dsp.lisp`. Its SHA-256 is
`90f2b929e75735cdf3a5c85bebc61cfc05ab53217cda1f0b5a7e9dfc77be6f9e`.
Temporary validation source: `/tmp/validate_spectral_tamer.rs`.

That initial draft was deliberately not installed. After the compiler fix, the
detector filters were implemented with explicit normalized coefficients and
host-rate frequency calculation: the legacy compiler biquad has a different
mode/gain contract and a hard-coded 44.1 kHz frequency scale (upstream follow-up
`dgen-1n1`). The tested effect is now installed locally as described above.
Exact reference-filter parity and subjective sound quality remain unverified;
automatic custom-effect latency compensation is not yet supported.
