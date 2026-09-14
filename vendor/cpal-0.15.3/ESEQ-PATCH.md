# Eseq macOS workgroup access

Source: the crates.io cpal 0.15.3 distribution (Apache-2.0, LICENSE unchanged).

Local changes are limited to the macOS backend:

- `Stream::audio_workgroup()` reads the actual output AudioUnit's workgroup on
  a control thread. CoreAudio returns an owned reference, released by the host.
- The internal property-listener callback requires `Send`. Its existing caller
  already requires a `Send` error callback; this makes the concrete macOS stream
  safely shareable with the control thread without an unsafe `Send` override.

The sequencer monitors this property outside the audio callback, joins its own
helpers, and releases references after membership changes are acknowledged.
No audio rendering or non-macOS backend behavior is changed. Keep these two
extensions explicit when updating CPAL; do not replace them with layout casts
into CPAL's private stream representation.
