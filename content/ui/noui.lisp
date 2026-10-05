; Bare root for `metal_seq noui` / `eseq -noui FILE` (eseq-750i).
;
; The full distro root (ui/main.lisp) assembles the DAW: step grid, mixer,
; fx panels, arrangement, browser. This root keeps only what a plain Lisp
; editing / live-coding session wants underneath it — the theme, the shared
; widget materials and binding helpers, the sequencer core state and the MIDI
; mapping table — so the audio engine, the sequencer natives, user init.lisp
; and imported packages all still work, with no DAW buffers registered.
;
; Anything else is one `(import …)` away from the session itself.

(load "@/ui/themes.lisp")
(seq-theme-mac-osx-dark)
(import eseq.materials)
(import eseq.bindings)
(import eseq.seq-core-state)
(import eseq.midi)
