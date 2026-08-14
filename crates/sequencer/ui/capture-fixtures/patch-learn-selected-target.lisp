;; Selected Patch Learn target row with a database display title long enough
;; to exercise the bounded label used beside the Change button.
(capture-project
  (track :instrument "core/drift"))

(effect-buffer "*patch-learn-selected-target*"
  (box :width :fill :height :fill :background-color :buffer-bg :padding 0.55
    (eseq.browser/sample-browser-widget
      true
      "samples/72a634c18e3035e7526edd85d31cb675806a3834b48ed7c53d8.wav"
      "Tim Maia – Est Dificil (Original Studio Recording 1971)")))
