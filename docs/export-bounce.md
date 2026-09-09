# Export an arrangement

Save the project, then run **M-x export-song** in the app. The command opens a
modal with an editable filename, whole-arrangement or explicit beat range,
sample rate, and tail duration. The Lisp entry is
`(eseq.export-song/export-song)`.

Exports go to the app's recordings directory (`.local/recordings` in a checkout).
The suggested name is `Project (1).wav`, then the next available number.
Existing files are never replaced by the dialog. Progress and cancellation stay
available during export; completion offers **Show in Finder** on macOS or
**Open folder** on Linux. Reopening the command during a job shows that job.
No toolbar button or actions menu is added.

The dialog exports the **saved project**, just like the command below. It copies
saved arrangement data into the job when export begins; later project saves do
not change that job. Unsaved live edits are not included.

You can also run from the repository root:

```sh
cargo run --release -p sequencer --bin eseq_export -- \
  --project .local/projects/bc-kicktest.json \
  --out /tmp/bc-kicktest.wav
```

Replace the project and output paths as needed. The command opens a saved project
in its own process without opening an audio device. It writes the arrangement's
stereo master mix as 32-bit float WAV, at 48 kHz with a 10-second tail by default.


Options:

- `--sample-rate 44100`: choose the output sample rate.
- `--tail 15`: allow 15 seconds for releases and effects after the range ends.
- `--start-beat 16 --end-beat 32`: export a beat range; both flags are required.
  The worker renders the preceding arrangement to establish effect state.
- `--replace`: explicitly allow replacing an existing output file. Without it,
  an existing destination is protected.
- `--cancel-file PATH`: cancel when this file exists, for scripted callers.

Press Ctrl-C to cancel. Output is published only after a successful render;
cancellation and rendering errors preserve an existing destination. Progress is
printed while rendering, followed by frame count and peak level. If audio remains
at the end of the tail, the command recommends a longer tail.

This command reads saved project data and current instrument/effect libraries
and sample files on disk. Save edits before exporting. It does not capture unsaved
live instrument code, live overrides, or the current DSP state. Reopened sample
files may differ from buffers already loaded in the app. Missing referenced
samples and effect assets fail export; explicitly blank sampler sources remain blank.
Failures identify the stage: validation, preparation, rendering, writing or
publication. Loading, analysis and song/latency preparation complete before the
worker creates the temporary output file.

The worker requires fixed processing latency and compensation throughout the
arrangement. Unsupported topology or compensation changes fail without publishing
a partial WAV. Full in-app source capture and broader arrangement acceptance
coverage remain tracked in the bounce epic.
