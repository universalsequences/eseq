;; Real instrument preset list, rendered through the production project-backed
;; browser so preset row icon spacing can be inspected.
(capture-project
  (track :instrument "core/wavetable"))

(def capture-after-sync ()
  (set! sbrowser-tab "presets"))
