;; The alez.lanes package over the Seq tile: tracks of different lengths (one
;; wrapping onto a second row), p-locked steps, and steps carrying every
;; parameter the tabs mark (velocity, delay, duration, retrig, transpose).
(capture-project
  (track :sampler :name "Kick" :num-steps 16 :steps (0 4 8 12 14)
         :step-params ((14 :velocity 0.5) (4 :delay 0.3) (8 :retrig 3)
                       (0 :sync 5) (4 :sync 4) (8 :sync 3) (12 :sync 7) (14 :sync 6)))
  (track :sampler :name "Snare" :num-steps 12 :steps (4 10 11)
         :step-params ((11 :velocity 0.35) (10 :retrig 2) (4 :duration 0.4)))
  (track :instrument "factory:Synths/Digi Drift" :name "Hat" :num-steps 32
         :steps (0 2 4 6 8 10 12 14 17 21 26 30)
         :step-params ((2 :velocity 0.5) (6 :velocity 0.25) (10 :velocity 0.75) (14 :delay 0.6)
                       (17 :duration 0.5) (21 :retrig 7))
         :instrument-locks ((4 "lp_freq" 2000) (5 "lp_freq" 6000) (12 "lp_freq" 12000)))
  (track :instrument "factory:Synths/Digi Drift" :name "Bass line long name" :num-steps 16
         :steps ((0 -12) (3 -12) (6 9) (8 -24) (11 5) (14 -7))
         :step-params ((3 :velocity 0.7) (11 :velocity 0.8) (0 :duration 2) (8 :duration 3))))

(import alez.lanes.ui)
