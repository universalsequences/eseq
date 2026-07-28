;; Transport clock regression fixture: the session scheduler has elapsed for
;; 14 bars, but Song mode is positioned at bar 129. The visible clock must use
;; the arrangement position rather than the reset-on-Play session counter.

(capture-project
  (track :sampler :name "Sampler"))

(effect-buffer "*transport-clock-preview*"
  (box :background "transport-led-bg" :padding 0.5 :height 4 :width 12
    (transport-clock
      :playhead 222
      :song-position-beats 513.5
      :use-song-position true
      :font-size 15 :width 10 :height 1.2
      :color '(rgba 0.85 0.85 0.85 1)
      :bg :transparent)))
