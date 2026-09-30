# PM Bongos reference material

Identified from the sampler track "A4 Bird Of Prey.flac" in `.local/projects/bongo-breaks.json`
(`.local/samples/610ba5adaf1687e2350062b641f38b9dbddc1685f3db2de201439871576d8d06.wav`, SHA256 `610ba5adaf1687e2350062b641f38b9dbddc1685f3db2de201439871576d8d06`). The recording is a commercial
record in the local sample library: only fitted modal coefficients are stored;
no PCM, recorded phase or spectral frame ships with the instrument.

Twelve strokes repeat in every octave. The C4 octave plays them at the recorded
pitch; each octave up or down transposes the whole kit by an octave.

| Key (any octave) | Stroke | Contact events (ms) |
| --- | --- | --- |
| C | Low Open | 0.0, 29.0, 69.5 |
| C# | Slap + Ghost | 0.0, 4.0, 48.5 |
| D | Low Ghost | 0.0, 39.5, 77.5 |
| D# | Pressed Tone | 0.0, 41.5 |
| E | Mid Open Flam | 0.0, 11.0, 37.5 |
| F | Mid Open | 0.0, 22.0, 67.0 |
| F# | Mid Open Short | 0.0, 30.0, 64.5 |
| G | Mid Ghost | 0.0, 49.0, 77.5 |
| G# | Low Open 2 | 0.0, 24.5, 79.5 |
| A | Slap Low Flam | 0.0, 34.5, 54.5 |
| A# | High Low Muted | 0.0, 9.0, 16.5 |
| B | High Slap Flam | 0.0, 14.0, 30.0 |

The loop's second open mid tone (mid-open-2) is a near-duplicate and is played by Mid Open (F).

Analysis, generator and comparison: `tools/pm-bongos/`.
