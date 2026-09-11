;; Package view rendered through the production project and widget path:
;; four tracks with mixed velocities, a retrig, durations and instrument
;; p-locks, so the tracker shows Note/Vol plus a p-lock column per lock.
(capture-project
  (track :sampler :name "Kick" :num-steps 16 :steps (0 4 8 12 14)
         :step-params ((14 :velocity 0.5)))
  (track :sampler :name "Snare" :num-steps 16 :steps (4 12 15)
         :step-params ((15 :velocity 0.35) (12 :retrig 2)))
  (track :instrument "factory:Synths/Digi Drift" :name "Hat" :num-steps 16
         :steps (0 2 4 6 8 10 12 14)
         :step-params ((2 :velocity 0.5) (6 :velocity 0.5) (10 :velocity 0.5) (14 :velocity 0.5))
         :instrument-locks ((0 "lp_freq" 2000) (4 "lp_freq" 6000) (8 "lp_freq" 12000) (12 "lp_freq" 18000)))
  (track :instrument "factory:Synths/Digi Drift" :name "Bass" :num-steps 16
         :steps ((0 -12) (3 -12) (6 -9) (8 -12) (11 -5) (14 -7))
         :step-params ((3 :velocity 0.7) (11 :velocity 0.8) (0 :duration 2) (8 :duration 3))
         :instrument-locks ((6 "lp_freq" 900) (11 "lp_freq" 1400))))

(import alez.tracker.ui)

;; Show a process lane as a column on the Bass track and open the Hat
;; track's column picker so the nested device groups are visible.
(def capture-after-sync ()
  (do
    (alez.tracker.ui/toggle-column 3
      (alez.tracker.ui/lane-key (nth (nth SEQ.track-process-lanes 3) 0)))
    (alez.tracker.ui/open-column-menu 2 (dict :col 62 :row 8))))
