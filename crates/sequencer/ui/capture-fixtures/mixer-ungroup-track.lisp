;; Open a member track's context menu in the production mixer.
;; Capture with --buffer mixer --width 1600 --height 600.
(capture-project
  (track :sampler :name "Kick")
  (track :sampler :name "Snare")
  (track :sampler :name "Bass")
  (group 0 1))

(def capture-after-sync ()
  (eseq.mixer/open-strip-menu (dict :at (dict :col 30 :row 12)) (eseq.kinds/track 1) nil))
