;; Mesh widget capture (docs/defwidget-vertex-stage.md): mode-mesh-view
;; drawing a plate mode from generated geometry, then two eseq-modes files
;; from scripts/gen_modes_fixture.py (a plate and a bell-like shell), one
;; through the widget's :src and one through a top-level mesh-load, both
;; resolved beside this file. Each renders offscreen with a depth buffer and
;; is composited as an image.
;;
;;   metal_seq capture --script crates/sequencer/ui/capture-fixtures/mode-mesh-view.lisp \
;;     --buffer mode-mesh-view --width 1500 --height 1100 --out /tmp/mode-mesh-view.png

(capture-project
  (track :sampler :name "Sampler"))

(import eseq.modal.mesh-view :as mv)

(def plate (mesh-geometry (mv/plate-geometry)))
(def bell (mesh-load "modes/bell.modes.json"))

(effect-buffer "*mode-mesh-view*"
  (v-stack :gap 1 :padding 1
    (h-stack :gap 2
      (v-stack
        (label "generated plate, mode (3,1)")
        (mv/mode-mesh-view :key "generated-plate" :geometry plate :amps (list 0 0 0 1)
                           :scale 0.15 :width 34 :height 14 :background "#1b1d22"))
      (v-stack
        (label "plate.modes.json, modes 1 + 3")
        ;; The widget itself, so :src resolves beside this file.
        (mv/mode-mesh-face :key "file-plate" :src "modes/plate.modes.json" :amps (list 1 0 0.6)
                           :scale 0.15 :yaw 2.4 :pitch 0.6 :zoom 1 :width 34 :height 14
                           :background "#1b1d22")))
    (h-stack :gap 2
      (v-stack
        (label "bell.modes.json, mode (2,1) + (3,1)")
        (mv/mode-mesh-view :key "file-bell" :geometry bell :amps (list 1 0.5)
                           :scale 0.1 :pitch 0.45 :width 34 :height 14 :background "#1b1d22"))
      (v-stack
        (label "generated membrane, mode (1,1)")
        (mv/mode-mesh-view :key "generated-membrane" :geometry (mv/membrane-geometry)
                           :amps (list 0 1) :scale 0.2 :width 34 :height 14 :background "#1b1d22")))))
