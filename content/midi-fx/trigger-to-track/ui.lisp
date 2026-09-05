;; Trigger-to-track panel: the destination track this effect fires on every
;; incoming trigger, plus a readout of what that means.
(def ttt-live-track ()
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param "track")))
    (if p (round (eseq.effects.custom-ui-runtime/custom-ui-param-value p)) 0)))

(def ttt-target-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium "TARGET" (eseq.effects.custom-ui-lego/ui-accent-blue)
    (h-stack :gap 0.6 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "track" "track" 4.7 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
      (v-stack :gap 0.2 :width 11.0 :align :start
        (label "every trigger on this track" :font-size 8.4 :width 11.0 :color :dim :bg :transparent)
        (label "also fires the target track" :font-size 8.4 :width 11.0 :color :dim :bg :transparent)
        (label "(a track never fires itself)" :font-size 8.0 :width 11.0 :color :dim :bg :transparent)))))

(def ttt-readout-block ()
  (let ((track (ttt-live-track)))
    (eseq.effects.custom-ui-lego/ui-readout-block-small "FIRES" (eseq.effects.custom-ui-lego/ui-accent-blue)
      (subtree :key (str "ttt-readout-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" track)
        (h-stack :gap 0.4 :align :center
          (label "fires" :font-size 8.6 :color :dim :bg :transparent)
          (label (str "track " track) :font-size 10.5 :color :blue :bg :transparent))))))

(def-midi-fx-ui
  (h-stack :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (ttt-target-block)
      (ttt-readout-block))))
