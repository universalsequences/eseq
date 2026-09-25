;; Drum rack pad role tags (docs/rack-groove-spec.md, "Pad roles"): four pads
;; moved onto the standard layout (C4 kick, D4 snare, F#4 closed hat) plus a
;; pad on F4 (pad note 5, the layout's low tom) explicitly tagged Clap.
;; Inferred tags draw dim, the explicit one bright.
;;
;;   metal_seq capture --script crates/sequencer/ui/capture-fixtures/rack-pad-roles.lisp \
;;     --buffer fx --width 2400 --height 420 --out /tmp/rack-pad-roles.png
(capture-project
  (track :sampler :name "Kick" :steps (0 8))
  (track :sampler :name "Snare" :steps (4 12))
  (track :sampler :name "Hat" :steps (0 2 4 6 8 10 12 14))
  (track :sampler :name "Clap" :steps (12))
  (drum-rack 0 1 2 3))

(def capture-after-sync ()
  (let ((gidx 0))
    (do
      (eseq.drum-rack-v2/move-pad-to-note gidx -36 0)
      (eseq.drum-rack-v2/move-pad-to-note gidx -35 2)
      (eseq.drum-rack-v2/move-pad-to-note gidx -34 6)
      (eseq.drum-rack-v2/move-pad-to-note gidx -33 5)
      (host-command "set-rack-pad-role"
        (dict :group-id (eseq.drum-rack-v2/group-id gidx) :pad-note 5 :role "clap"))
      (set! eseq.seq-core-state/selected-bus (eseq.drum-rack-v2/bus-index gidx)))))
