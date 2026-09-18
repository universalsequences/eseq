;; Shared shader widgets and selected-effect actions for the FX strip.
(module eseq.effects.panel-widgets)

(import eseq.effects.track-panels :as tp)

(import eseq.effects.process-panel :as pp)

(export select-effect
        select-midi-effect
        select-bus-effect
        select-rack-effect
        has-selected-bus?
        delete-selected-effect)

;; Aliases for Rust tests that eval the old flat spellings
;; (src/ui/state_values/tests.rs). The buffers.lisp flat edges
;; (fx-has-selected-bus?) retired with eseq.effects.buffers, which imports
;; this module.

(def select-effect (slot)
  (do
    (pp/clear-selection)
    (seq-set-delete-target :fx-effect (dict :chain "audio" :slot slot))))

(def select-midi-effect (slot)
  (do
    (pp/clear-selection)
    (seq-set-delete-target :fx-effect (dict :chain "midi" :slot slot))))

(def select-bus-effect (bus slot)
  (do
    (pp/clear-selection)
    (seq-set-delete-target :fx-effect (dict :chain "bus" :bus bus :slot slot))))

(def select-rack-effect (track rack-slot effect-slot)
  (do
    (pp/clear-selection)
    (seq-set-delete-target :fx-effect
      (dict :chain "rack"
            :track track
            :rack-slot rack-slot
            :effect-slot effect-slot))))

(def has-selected-bus? ()
  (and (>= eseq.seq-core-state/selected-bus 0)
       (< eseq.seq-core-state/selected-bus (len SEQ.bus-names))
       (< eseq.seq-core-state/selected-bus (len SEQ.bus-effects))))

(def delete-selected-effect ()
  (if (pp/delete-selected)
    true
    (if (tp/plock-row-selected?)
      (tp/delete-selected-plock-row)
      (seq-delete-active-target))))

(defwidget fx-panel-bg
  :width 1 :height 1
  :state (selected header-r header-g header-b selected-header-r selected-header-g selected-header-b)
  :shader
  (let ((panel-radius (min (* 3 (fwidth y)) (* 0.5 (min width height))))
      (panel (sdf/rounded-rect (* 1 width) (* 1 height) (* 2 panel-radius)))
      ;; Use derivatives to convert a real pixel height into the shader's
      ;; normalized/SDF y-space. This keeps the header bar visually constant
      ;; as panels get taller/shorter.
      (header-h (* 35 (fwidth y)))
      (header-bottom (+ (- height) header-h))
      (header-shape (max panel (- y header-bottom))))
    (sdf/layer
      (sdf/fill
        panel
        (material
          :color
          (let ((header-aa (max (fwidth header-shape) (fwidth y)))
                (border-w (max (* 1.5 (fwidth d)) (fwidth y)))
                (body input-color)
                (header (rgba header-r header-g header-b 1.0))
                (base
                  (mix header body (smoothstep 0 header-aa header-shape))))
            base)))
      (if selected
        (sdf/fill header-shape
          (material :color (rgba selected-header-r selected-header-g selected-header-b 1.0)))
        (rgba 0 0 0 0)))))

(defwidget compile-progress
  :width 12 :height 0.3
  :state (active)
  :shader
  (if (= active 0)
    (rgba 0 0 0 0)
    (let ((bar-w 0.3)
          (pos (fract (* 0.5 itime)))
          (bar-x (- (* pos (+ 1 bar-w)) (/ bar-w 2)))
          (d-bar (- (abs (- x bar-x)) (/ bar-w 2)))
          (bg (sdf/rounded-rect width height 0.06))
          (mask (max bg (- d-bar))))
      (sdf/layer
        (sdf/fill bg
          (material :color (rgba 0.15 0.15 0.17 1)))
          (sdf/fill mask
          (material :color
            (mix
              (rgba 0.3 0.5 1.0 1)
              (rgba 0.2 0.35 0.8 1)
              (smoothstep -0.02 0.02 d-bar))))))))
