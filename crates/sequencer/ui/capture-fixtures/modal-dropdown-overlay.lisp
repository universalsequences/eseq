;; Nested-overlay fixture: a dropdown INSIDE an open modal, forced open via
;; capture-click-widgets. The menu must draw on top of the modal panel (the
;; modal captures primitives pushed during its subtree recursion and re-appends
;; them after its own content).
;;
;;   metal_seq capture --script crates/sequencer/ui/capture-fixtures/modal-dropdown-overlay.lisp \
;;     --buffer modal-dropdown-preview --width 900 --height 600 --out /tmp/modal-dropdown-overlay.png

(capture-project
  (track :sampler :name "Sampler"))

(def capture-click-widgets (list "dropdown"))

(effect-buffer "*modal-dropdown-preview*"
  (v-stack :gap 1 :padding 1
    (label "underlay content behind the modal")
    (box :background-color '(rgba 0.25 0.2 0.3 1) :width 40 :height 12
      (label "underlay panel"))
    (modal :is-open true :title "Modal with a dropdown"
      (v-stack :gap 0.6
        (label "The open menu below must cover the modal's own content")
        (dropdown
          :width 18
          :height 1.4
          :options '("plate" "hall" "quad" "mod" "spring" "shimmer")
          :value "plate")
        (label "content row under the dropdown")
        (label "another content row the menu should overlap")))))
