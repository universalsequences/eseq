;; Overlay-in-capture regression fixture: forces a dropdown menu open via a
;; synthetic click (capture-click-widgets) so the offscreen render must draw
;; the drained overlay primitives. Before the capture overlay stage existed,
;; this PNG showed only the closed trigger — the menu was silently discarded.
;;
;;   metal_seq capture --script crates/sequencer/ui/capture-fixtures/dropdown-open-overlay.lisp \
;;     --buffer dropdown-overlay-preview --width 900 --height 600 --out /tmp/dropdown-open-overlay.png

(capture-project
  (track :sampler :name "Sampler"))

(def capture-click-widgets (list "dropdown"))

(effect-buffer "*dropdown-overlay-preview*"
  (v-stack :gap 1 :padding 1
    (label "dropdown overlay capture check")
    (dropdown
      :width 14
      :height 1.4
      :options '("plate" "hall" "quad" "mod" "spring" "shimmer")
      :value "plate")
    (box :background '(rgba 0.2 0.25 0.3 1) :width 34 :height 10
      (label "underlay content — the open menu must cover part of this box"))))
