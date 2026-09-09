# Factory samples

76 samples, 15,030,294 bytes of audio (14.33 MiB), available offline on first
launch. This package ships inside the application. Startup imports it into the
normal content-addressed sample store, and the Samples browser exposes its
titles, tags, and a concise **Factory** chip and row badge. Filtering retains the
stable `pkg:universalsequences.factory-samples` identity. Search `808`, `piano`,
`salamander`, or `vcsl`; filter percussion by `kick`, `snare`, `closed hat`, etc.

| Collection | Selection | License / credit |
| --- | --- | --- |
| TR-808 Sound Sample Set 1.0.0 | 34 hits: all 16 voices, plus kick/snare/cymbal knob variations | CC0-1.0, Michael Fischer, via TidalCycles |
| Salamander Grand Piano V3 | 30 velocity-8 notes, A0–C8 | CC BY 3.0, Alexander Holm |
| Versilian Community Sample Library | 12 acoustic hits: bass drum, snare, hats, tambourine, claves, shaker, crash | CC0-1.0, Versilian Studios LLC |

The piano is a collection of individual pitched samples, not an SFZ instrument
or a velocity-layered piano preset. Note names identify the recorded pitch.
The selection covers every minor third from A0 to C8, including D#5
(which Strudel's piano map omits). The existing
sampler can play and transpose any of these notes. No SFZ articulation,
release-noise, or automatic key-zone mapping is implied.

Audio bytes are unchanged from the pinned upstream files. Salamander is the
compact MP3 conversion distributed by Strudel/dough-samples; these are not the
original 24-bit recordings. The application decodes the selected files to WAV
in its writable store. No additional trimming, normalization, or resampling
was applied to the bundled payloads.

## Attribution and evidence

**Salamander Grand Piano V3 by Alexander Holm**, licensed under
[Creative Commons Attribution 3.0 Unported](https://creativecommons.org/licenses/by/3.0/).
[Original collection](https://archive.org/details/SalamanderGrandPianoV3).
Selected velocity-8 MP3 conversion distributed by
[dough-samples](https://github.com/felixroos/dough-samples/tree/9eacfc86ec4393e68a463ff52b01c19cfaa77f38).
The app's About dialog also carries this attribution, source, and license link.

The 808 selection comes specifically from
[TidalCycles/sounds-tr808-fischer](https://github.com/tidalcycles/sounds-tr808-fischer/tree/85fbecf1bec32553395625ea659e2a56dfd7c0e1),
whose repository supplies CC0 and Michael Fischer's original recording notes.
Credit belongs to the recording author, not a later repository contributor.

[VCSL](https://github.com/sgossner/VCSL/tree/c1ea7bcc3c7309650ab0da9d15c9cd1fbc4a4c7e)
explicitly dedicates the samples to CC0, including use in commercial software.

`provenance.json` records every exact shipped audio and notice file, immutable
upstream URL, SHA-256, byte length, collection, title, and tags. `licenses/`
preserves upstream README notices and full license texts. `samples.jsonl`
connects each sample to the same source/creator/license records in the app DB.
These sample licenses are separate from the application's software license.

## Maintenance

Run `python3 scripts/factory_samples.py` from the repository to verify all
payloads and their manifest metadata offline. Add `--fetch` to restore missing
files from their pinned URLs; existing mismatched files fail verification.
The macOS packaging script runs the same verification before building.
All payloads are checked in, so normal installation needs no download.

Scripts may `(import universalsequences.factory-samples.library)` and read `samples`, a
title-to-portable-reference dictionary. Projects persist `samples/<hash>.wav`,
independent of the bundle location. Reindexing this package with the generic
`eseq package index` command replaces curated metadata; use the audited manifest.

## Sources investigated but not included

Research date: 2026-09-09. Strudel's
[sample documentation](https://strudel.cc/learn/samples/) points to
`geikha/tidal-drum-machines`, but that repository's root does not establish a
redistribution license for its individual collections. `Dirt-Samples` likewise
contains mixed sources, including sampled music; it is not blanket CC0.

The [Oramics TR-909 Detroit README](https://github.com/oramics/sampled/blob/84d3405e107ad52986e7ca99af6a4ed3efe205de/DM/TR-909/Detroit/README.md)
and SP README trace those sounds to F9 Audio,
without an explicit CC0 grant. Other apps' descriptions of them as public
domain are not source evidence. The [Hyperreal/Rob Roy 909 notice](https://github.com/fluid-music/open-drums/blob/475cc3314fe06f6d1af02e9790ad9707c1f2b26b/tr-909/TR909all/TR909SET.TXT)
has a no-distribution-for-profit restriction. None of these 909 sets is included.
The initial library favors verified sources over presumed permission; more
machine kits require an explicit license or permission from the recording owner.
