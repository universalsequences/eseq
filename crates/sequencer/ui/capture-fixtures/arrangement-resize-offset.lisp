;; Arrangement clip preview with a live left-edge trim at an off-cycle beat.
;; The four-beat pattern is audible over [2, 6), so its source phase at beat
;; 2 is step 8. The capture's live start-edge drag trims the clip to beat 3;
;; the visible notes must stay at beats 3/4/5 rather than restarting or
;; stretching to the new three-beat span.

(capture-project
  (track :sampler :name "Offset Pattern" :steps (0 4 8 12)))

(def-song "resize-offset"
  (at 0 :scene 0 :patterns ((0 0)))
  (at 2 :scene 0 :patterns ((0 1)))
  (at 6 :scene 0 :patterns ((0 0)))
  :end 16)

(def capture-after-sync ()
  (do
    (eseq.seq-panels/seq-open-arrangement)
    (let ((view eseq.arrangement/arr-view)) (set! view.duration 16))
    (eseq.arrangement/set-view-start 0 16)
    (let ((t (nth (eseq.kinds/tracks) 0)))
      (let ((c (nth t.clips 0)))
        (eseq.arrangement/track-action 0
          (dict :type :resize-item-absolute :id c.cid :ids (list c.cid) :edge :start :time 3))))))
