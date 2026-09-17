;; Custom instrument and effect controls share a panel but retain distinct
;; parameter owners when a step lock causes a control subtree to rerender.
(capture-project
  (track :instrument "factory:Physical Models/PM Piano"
    :audio-fx ("lexilush" "Compressor")))
