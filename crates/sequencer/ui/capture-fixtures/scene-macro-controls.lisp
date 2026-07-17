(capture-project
  (track :sampler :name "Drums"))

(host-command "macro-create-scene" (dict :name "Push Scene" :target-scene 0))

(effect-buffer "*scene-macro-capture*"
  (box :width :fill :height :fill :padding 1.0 :background-color :buffer-bg
    (scene-macro-controls :macro 1)))
