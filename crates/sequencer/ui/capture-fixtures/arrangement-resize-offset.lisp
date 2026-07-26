;; Arrangement clip preview with a live left-edge trim at an off-cycle beat.
;; The four-beat pattern is audible over [2, 6), so its source phase at beat
;; 2 is step 8. The capture ghost trims the clip to beat 3; the visible notes
;; must stay at beats 3/4/5 rather than restarting or stretching to the new
;; three-beat span.

(capture-project
  (track :sampler :name "Offset Pattern" :steps (0 4 8 12)))

(def-song "resize-offset"
  (at 0 :scene 0 :patterns ((0 0)))
  (at 2 :scene 0 :patterns ((0 1)))
  (at 6 :scene 0 :patterns ((0 0)))
  :end 16)

(def capture-after-sync ()
  (do
    (seq-open-arrangement)
    (set! arrangement-view-start 0)
    (set! arrangement-view-duration 16)
    (let ((clip (nth (nth SEQ.song-lanes 0) 0)))
      (set! arrangement-ghost
        (dict
          :kind :track-resize
          :track 0
          :clip-id (get clip :clip-id)
          :edge :start
          :time 3)))))
