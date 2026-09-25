;; Drum rack panel Groove section (docs/rack-groove-spec.md, "UI"): a
;; three-pad rack whose members carry a played, off-grid take (step Delay
;; nudges; the kick's step-7 hit is "very late", i.e. early on step 8). The
;; hook extracts a groove through the production host command, quantizing the
;; source, and selects the rack so *fx* shows its panel.
;;
;;   metal_seq capture --script crates/sequencer/ui/capture-fixtures/rack-groove-panel.lisp \
;;     --buffer fx --width 2400 --height 420 --out /tmp/rack-groove.png
(capture-project
  (track :sampler :name "Kick" :steps (0 7 10)
    :step-params ((0 delay 0.02) (7 delay 0.85) (10 delay 0.06)))
  (track :sampler :name "Snare" :steps (4 12)
    :step-params ((4 delay 0.12) (12 delay 0.18)))
  (track :sampler :name "Hat" :steps (0 2 4 6 8 10 12 14)
    :step-params ((0 delay 0.04) (2 delay 0.3) (4 delay 0.08) (6 delay 0.26)
                  (8 delay 0.05) (10 delay 0.34) (12 delay 0.1) (14 delay 0.22)))
  (drum-rack 0 1 2))

(def capture-after-sync ()
  (do
    (eseq.drum-rack-v2/extract-groove (eseq.drum-rack-v2/group-id 0)
      "Dilla take" 1 "1/16" true)
    (set! eseq.seq-core-state/selected-bus (eseq.drum-rack-v2/bus-index 0))))
