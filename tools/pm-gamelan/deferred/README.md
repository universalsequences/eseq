# Kempul identification evidence

These are measurements of the 16 `kempul-pelog` recordings and a comparison
from a **rejected**, uninstalled passive-model candidate. They are retained
to make the selection decision reviewable. They are not current factory
validation reports, and their DSP hash does not name an installed instrument.

The candidate used 32 modal slots; notes 5, 6, 7 and 1h mapped to MIDI
45, 46, 48 and 50. Softest, softer, medium and harder mapped to velocities
0.25, 0.5, 0.75 and 1.0. Identification used a 1.2-second spectral window,
up to eight seconds of envelope fitting and a nominal 0.6 ms contact pole.
All four strengths shared modal frequencies, damping and radiation poles.

Its median level error from 25 ms to one second was 1.16 dB, but the 90th
percentile was 5.54 dB and the maximum was 10.58 dB. That disagrees too much
with the measured early dynamics. Independent window spectra also revealed
strike-dependent frequencies and close beating components. A good sustained
spectrum alone did not justify factory inclusion.

The follow-up is **eseq-mpkt**. Investigate the physical coupling and
nonlinearity before attempting another reduction. The source license and
credits in the parent directory apply to these measurements too.
