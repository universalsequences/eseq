;; Mixed pattern lengths: 4, 16 and 8 steps. Shorter tracks repeat down the
;; shared grid as ghost rows in their own track tint; the loop restart row
;; is tinted a little stronger.
(capture-project
  (track :sampler :name "Kick" :num-steps 4 :steps (0 2))
  (track :sampler :name "Snare" :num-steps 16 :steps (4 12))
  (track :sampler :name "Hat" :num-steps 8 :steps (0 2 4 6)))
(import alez.tracker.ui)
