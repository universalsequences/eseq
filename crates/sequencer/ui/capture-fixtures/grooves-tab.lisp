;; Grooves browser tab (docs/rack-groove-spec.md, "Rev 2 UI"; eseq-groove.11):
;; two drum racks, the first's played take (pads on the standard layout) extracted into the project pool
;; (quantize source on, so it plays it) and applied to the second too. The
;; hook opens the tab with every groove's instances expanded and the take
;; selected, so the tree shows In use / Project / Library / Factory, the two
;; instance rows, and the heatmap preview below.
;;
;;   metal_seq capture --script crates/sequencer/ui/capture-fixtures/grooves-tab.lisp \
;;     --buffer samples --width 700 --height 1100 --out /tmp/grooves-tab.png
(capture-project
  (track :sampler :name "Kick" :steps (0 7 10)
    :step-params ((0 delay 0.02) (7 delay 0.85) (10 delay 0.06)))
  (track :sampler :name "Snare" :steps (4 12)
    :step-params ((4 delay 0.12) (12 delay 0.18)))
  (track :sampler :name "Hat" :steps (0 2 4 6 8 10 12 14)
    :step-params ((0 delay 0.04) (2 delay 0.3) (4 delay 0.08) (6 delay 0.26)
                  (8 delay 0.05) (10 delay 0.34) (12 delay 0.1) (14 delay 0.22)))
  (track :sampler :name "Kick 2" :steps (0 8))
  (track :sampler :name "Hat 2" :steps (2 6 10 14))
  (drum-rack 0 1 2)
  (drum-rack 3 4))

(def capture-after-sync ()
  (do
    ;; Standard-layout pads (C4 kick, D4 snare, F#4 closed hat), so the
    ;; extracted rows record roles and the preview labels them by role.
    (eseq.drum-rack-v2/move-pad-to-note 0 -36 0)
    (eseq.drum-rack-v2/move-pad-to-note 0 -35 2)
    (eseq.drum-rack-v2/move-pad-to-note 0 -34 6)
    (eseq.drum-rack-v2/extract-groove (eseq.drum-rack-v2/group-id 0)
      "Dilla take" 1 "1/16" true)
    (host-command "set-rack-groove"
      (dict :group-id (eseq.drum-rack-v2/group-id 1) :key "pool:1"))
    (eseq.browser/select-tab "grooves")
    (set! eseq.grooves-tab/expand-instances true)
    (eseq.grooves-tab/show-groove "pool:1" "project/pool:1")
    (eseq.browser/refresh-buffer)))
