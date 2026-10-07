;; Drum rack pad role tags (docs/rack-groove-spec.md, "Pad roles"): a new
;; rack fills its home octave from C1, which is where the standard layout
;; (GM drum map, kick on C1) lives, so the untouched first pads infer Kick
;; (C1), Rim (C#1) and Snare (D1). The hat moves to F#1 (closed hat) and the
;; clap to F1 (the layout's low tom), explicitly tagged Clap. Inferred tags
;; draw dim, the explicit one bright.
;;
;;   metal_seq capture --script crates/sequencer/ui/capture-fixtures/rack-pad-roles.lisp \
;;     --buffer fx --width 2400 --height 420 --out /tmp/rack-pad-roles.png
(capture-project
  (track :sampler :name "Kick" :steps (0 8))
  (track :sampler :name "Rim" :steps (6 14))
  (track :sampler :name "Snare" :steps (4 12))
  (track :sampler :name "Hat" :steps (0 2 4 6 8 10 12 14))
  (track :sampler :name "Clap" :steps (12))
  (drum-rack 0 1 2 3 4))

(def capture-after-sync ()
  (let ((g (first (eseq.kinds/groups)))
        (pad (lambda (name) (first (filter (lambda (p) (= p.track.name name)) g.pads))))
        (hat (pad "Hat"))
        (clap (pad "Clap")))
    ;; Default pads: Kick C1, Rim C#1, Snare D1, Hat D#1, Clap E1.
    (set! hat.note -30)
    (set! clap.note -31)
    (set! clap.role "clap")
    (set! eseq.seq-core-state/selected-bus g.bus.index)))
