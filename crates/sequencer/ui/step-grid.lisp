;; ui/step-grid.lisp - Step grid UI for Metal Sequencer (the legacy *metal*
;; buffer). NOT loaded by ui/main.lisp — see the comment there: its
;; whole-list SEQ reads forced hidden-buffer reruns. Kept on disk for
;; reference and parsed (never evaluated) by the metal_seq parse tests.
;;
;; Converted in S3b wave 8. Notes specific to this file:
;;
;;   * `metal-track-r/-g/-b` are read from ui/materials.lisp's
;;     `slider-track-material` / `slider-track-muted-material` macro bodies,
;;     which expand at shader-compile time in a throwaway *implicit-module*
;;     compiler (spec §10 hazard h) — so they must stay reachable by their
;;     flat spelling. Identity compat aliases below do that, and the
;;     `metal-` prefix is deliberately NOT stripped: `track-r/-g/-b` are the
;;     `metal-track-tick` shader's own `:state` binders, and merging the two
;;     is exactly hazard (k).
;;   * `metal-track-tick` keeps its flat, unrenamed name (hazard e —
;;     `defwidget` is its own flat keyspace). ui/sequencer.lisp carries a
;;     verbatim copy under the same name on purpose; see the comment there.
;;   * The `:material` props below call ui/materials.lisp macros through
;;     their compat aliases (`aqua-slider-track-material`, …). Those bodies
;;     are auto-quoted and expand outside this module, so they must stay
;;     flat — do not requalify them and do not import eseq.materials.
;;   * `param-mode` stays bare: it is eseq.seq-core-state's `defstate`, and
;;     `defstate` resolves on the flat key through `state_bindings`.
;;   * `(set-buffer-mode-for "*metal*" "eseq.seq-grid-mode/seq-grid-mode")` at the bottom keeps
;;     its flat mode string: the reference qualifies against this module,
;;     misses, and lands on eseq.seq-grid-mode's identity alias rung.
;;   * step-grid-interactions.lisp / transport.lisp names (`step-index`,
;;     `page-button-width`, `pattern-control-style`, …) are left bare.
;;     transport.lisp is still a headerless owner (covered by the stage-3
;;     late-binding heal); step-grid-interactions.lisp converted in S3b wave 8
;;     and carries *identity* compat aliases, which a converted module's bare
;;     reference reaches through the base-name rung. All of them are
;;     write-once `def`s or functions.

(module eseq.step-grid)

(import eseq.seq-core-state :as core)
(import eseq.seq-grid-mode :as gm)
(import eseq.effects.state :as st)

;; Read bare from ui/materials.lisp's shader macro bodies (see above).

;; ── Main UI ──

(defstate metal-track-r 0.34)
(defstate metal-track-g 0.48)
(defstate metal-track-b 0.98)

(def %empty-track-fallback ()
  (box :width :fill :height :fill :padding 1 :h-align :center :v-align :center
    (v-stack :gap 0.35 :align :center
      (label "Select a sound to create a track"
        :font-size 14 :color :gray :bg :transparent)
      (label "Sampler, instruments, and projects are in the left browser."
        :font-size 10 :color :dark-gray :bg :transparent))))

(def %current-track-color ()
  (if (and (< SEQ.current-track (len SEQ.track-colors)) (>= SEQ.current-track 0))
    (nth SEQ.track-colors SEQ.current-track)
    (list 0.34 0.48 0.98)))

(def %track-color-r ()
  (nth (%current-track-color) 0))

(def %track-color-g ()
  (nth (%current-track-color) 1))

(def %track-color-b ()
  (nth (%current-track-color) 2))

(def %sync-track-color-state ()
  (do
    (set! metal-track-r (%track-color-r))
    (set! metal-track-g (%track-color-g))
    (set! metal-track-b (%track-color-b))))

(def %track-slider-fill ()
  (rgba (%track-color-r) (%track-color-g) (%track-color-b) 1.0))

(def %track-slider-muted-fill ()
  (rgba
    (+ (* (%track-color-r) 0.30) (* 0.08 0.70))
    (+ (* (%track-color-g) 0.30) (* 0.08 0.70))
    (+ (* (%track-color-b) 0.30) (* 0.12 0.70))
    0.50))

(def %track-slider-muted-dot ()
  (rgba
    (+ (* (%track-color-r) 0.28) (* 0.25 0.72))
    (+ (* (%track-color-g) 0.28) (* 0.25 0.72))
    (+ (* (%track-color-b) 0.28) (* 0.30 0.72))
    0.55))

(defwidget metal-track-tick
  :width 1.5 :height 1.5
  :state (active plocked selected track-r track-g track-b)
  :bindable (active plocked selected track-r track-g track-b)
  :shader
  (let ((sel-y (if (= selected 1) (* 0.1 (cos (* 3 itime))) 0)))
    (sdf/translate 0 sel-y
      (sdf/layer
        (sdf/fill (sdf/circle 1)
          (material
            :lighting (lighting :edge-min -0.35 :edge-max 0.5
              :light (vec3 0.0 -1.0 2.5) :shininess 32.0)
            :color
            (* (if (= active 1) 1 0.3)
               (eseq.materials/color
                 (rgba (* track-r 0.82) (* track-g 0.82) (* track-b 0.82) 1.0)
                 (rgba track-r track-g track-b 1.0)))))))))

(effect-buffer "*metal*"
  (if (= SEQ.num-tracks 0)
    (%empty-track-fallback)
    (do
    (%sync-track-color-state)

    (box :background-color :mixer-strip-bg :corner-radius 10
    (v-stack
      :padding 1.5
      :gap 0.1
      
      ; Param mode selector
      (h-stack :gap 0.5
        (box :width 8 :height 2
          :bg (if (= eseq.seq-core-state/param-mode 0) :blue :dark-gray)
          :on-click |x y r| (set! eseq.seq-core-state/param-mode 0)
          (label "vel" :font-size 12
            :color (if (= eseq.seq-core-state/param-mode 0) :white :gray)
            :bg :transparent))
        (box :width 8 :height 2
          :bg (if (= eseq.seq-core-state/param-mode 1) :green :dark-gray)
          :on-click |x y r| (set! eseq.seq-core-state/param-mode 1)
          (label "dur" :font-size 12
            :color (if (= eseq.seq-core-state/param-mode 1) :white :gray)
            :bg :transparent))
        (box :width 8 :height 2
          :bg (if (= eseq.seq-core-state/param-mode 2) :magenta :dark-gray)
          :on-click |x y r| (set! eseq.seq-core-state/param-mode 2)
          (label "aux_a" :font-size 12
            :color (if (= eseq.seq-core-state/param-mode 2) :white :gray)
            :bg :transparent))
        (box :width 8 :height 2
          :bg (if (= eseq.seq-core-state/param-mode 3) :yellow :dark-gray)
          :on-click |x y r| (set! eseq.seq-core-state/param-mode 3)
          (label "xpose" :font-size 12
            :color (if (= eseq.seq-core-state/param-mode 3) :white :gray)
            :bg :transparent))
        (box :width 8 :height 2
          :bg (if (= eseq.seq-core-state/param-mode 4) :red :dark-gray)
          :on-click |x y r| (set! eseq.seq-core-state/param-mode 4)
          (label "pan" :font-size 12
            :color (if (= eseq.seq-core-state/param-mode 4) :white :gray)
            :bg :transparent))
        (box :width 8 :height 2
          :bg (if (= eseq.seq-core-state/param-mode 5) :green :dark-gray)
          :on-click |x y r| (set! eseq.seq-core-state/param-mode 5)
          (label "syn" :font-size 12
            :color (if (= eseq.seq-core-state/param-mode 5) :white :gray)
            :bg :transparent))
        (h-stack :align :center :gap 0.35
          (dropdown :value SEQ.tp-timebase
            :options st/seq-timebase-options
            :on-change (lambda (v)
              (do
                (core/cool-off-follow)
                (if (seq-has-selection?)
                  (seq-plock-timebase v)
                  (seq-set-timebase v))))
            :width 6 :height 1.45 :font-size 10)))
    
    ; Step columns: vslider + aqua step toggle + step number
    (grid :cols 16 :col-width 4
      (each (range 0 core/page-size) |i|
        (let ((step (eseq.step-grid-interactions/step-index i))
              (visible (eseq.step-grid-interactions/step-visible? i)))
          (box :padding 0.25 :background (if visible (if (= (core/current-step) step) "cursor-highlight" nil) nil)
            :active true
            :selected true
            :on-click (lambda (evt)
              (if visible
                (do
                  (core/cool-off-follow)
                  (eseq.step-grid-interactions/set-track-cursor-step step)
                  (if (eseq.step-grid-interactions/selection-click? evt)
                    (eseq.step-grid-interactions/step-select-drag-start step evt)
                    (seq-clear-selection)))
                nil))
            :on-drag (lambda (evt)
              (if visible
                (eseq.step-grid-interactions/step-select-drag-over step evt)
                nil))
            (v-stack :align :center :gap 0.5
              (let ((step-on (and visible (nth SEQ.steps step))))
                (if step-on
                  (vslider :height 4
                    :width (if (= eseq.seq-core-state/param-mode 5) 2 1)
                    :min (gm/param-slider-min) :max (gm/param-slider-max)
                    :origin (gm/param-origin)
                    :value (gm/param-slider-value step)
                    :haptic-value (nth (gm/param-values) step)
                    :haptic-min (gm/param-min)
                    :haptic-max (gm/param-max)
                    :haptic-pivot-position (gm/param-haptic-pivot-position)
                    :haptic-pivot-value (gm/param-haptic-pivot-value)
                    :haptic-exponent (gm/param-haptic-exponent)
                    :items (if (= eseq.seq-core-state/param-mode 5) SEQ.sync-labels '())
                    :font-size 11
                    :color :white
                    :fill (%track-slider-fill)
                    :dot-color :dark-gray
                    :material (eseq.materials/slider-track-material)
                    :on-change (lambda (v)
                      (if visible
                        (do
                          (core/cool-off-follow)
                          (eseq.step-grid-interactions/set-track-cursor-step step)
                          (let ((value (eseq.step-grid-interactions/step-slider-param-value v)))
                          (eseq.step-grid-interactions/seq-set-step-param-from-step step (gm/param-keyword) value)))
                        nil)))
                  (vslider :height 4
                    :width (if (= eseq.seq-core-state/param-mode 5) 2 1)
                    :min (gm/param-slider-min) :max (gm/param-slider-max)
                    :origin (gm/param-origin)
                    :value (gm/param-slider-value step)
                    :haptic-value (nth (gm/param-values) step)
                    :haptic-min (gm/param-min)
                    :haptic-max (gm/param-max)
                    :haptic-pivot-position (gm/param-haptic-pivot-position)
                    :haptic-pivot-value (gm/param-haptic-pivot-value)
                    :haptic-exponent (gm/param-haptic-exponent)
                    :items (if (= eseq.seq-core-state/param-mode 5) SEQ.sync-labels '())
                    :font-size 11
                    :color :dim
                    :fill (%track-slider-muted-fill)
                    :dot-color (%track-slider-muted-dot)
                    :material (eseq.materials/slider-track-muted-material)
                    :on-change (lambda (v)
                      (if visible
                        (do
                          (core/cool-off-follow)
                          (eseq.step-grid-interactions/set-track-cursor-step step)
                          (let ((value (eseq.step-grid-interactions/step-slider-param-value v)))
                          (eseq.step-grid-interactions/seq-set-step-param-from-step step (gm/param-keyword) value)))
                        nil)))))
              (box
                :active (if visible (if (nth SEQ.steps step) 1 0) 0)
                :plocked (if visible (if (nth SEQ.step-has-plocks step) 1 0) 0)
                :selected (if visible (bind-seq-nth "selected-steps" step) 0)
                :background "aqua-button"
                :align :center :width 3 :height 1.5
                :on-mouse-down (lambda (evt)
                  (if visible
                    (eseq.step-grid-interactions/step-pointer-down step evt)
                    nil))
                :on-drag (lambda (evt)
                  (if visible
                    (eseq.step-grid-interactions/step-select-drag-over step evt)
                    nil))
                :on-mouse-up (lambda (evt)
                  (if visible
                    (eseq.step-grid-interactions/step-pointer-up step evt)
                    nil))
                :on-double-click (lambda (evt)
                  (if visible
                    (eseq.step-grid-interactions/step-double-click step evt)
                    nil))
                (metal-track-tick
                      :active (if visible (if (nth SEQ.steps step) 1 0) 0)
                      :plocked (if visible (if (nth SEQ.step-has-plocks step) 1 0) 0)
                      :selected (if visible (bind-seq-nth "selected-steps" step) 0)
                      :track-r (%track-color-r)
                      :track-g (%track-color-g)
                      :track-b (%track-color-b)))
              (label (if visible (str (+ step 1)) "")
                :font-size 10 :bg :transparent
                :active (if visible (bind-seq-nth "selected-steps" step) 0)
                :active-color :yellow
                :color :dim)
              (subtree :key (str "step-playhead-probe-" step)
                (step-playhead-dot
                  :active (bind-seq (str "playhead-active-" step)))))))))

    ; Step cursor info
    (h-stack :gap 1 :align :center
      (box :width 11.5 :height 1.3
        (label (fmt "Step {}  {}" (+ (core/current-step) 1) (gm/param-name))
          :font-size 11 :width 11.5 :color :dim :bg :transparent))
      (if (= eseq.seq-core-state/param-mode 5)
        (box :width 8 :height 1.3
          (label (gm/sync-current-label)
            :font-size 11 :color :white :bg :transparent))
        (number-picker :key "step-param-number-picker"
          :value (nth (gm/param-values) (core/current-step))
          :min (gm/param-min) :max (gm/param-max) :decimals (eseq.step-grid-interactions/param-decimals)
          :on-change (lambda (v)
            (do
              (core/cool-off-follow)
              (eseq.step-grid-interactions/seq-set-step-param-from-step
                (core/current-step)
                (gm/param-keyword)
                (eseq.step-grid-interactions/step-param-value v))))
          :width 8 :height 1.3 :font-size 11))
      (h-stack :gap 0.4 :align :center
        (box :background "pattern-pill-btn-bg" :width 2.5 :height 1.1 :active true
          :on-click |x y r| (gm/halve-track-pattern)
          (v-stack :align :center
            (label "-"
              :font-size 12
              :color :white
              :bg :transparent)))
        (box :background "pattern-pill-btn-bg" :width 2.5 :height 1.1 :active true
          :on-click |x y r| (gm/double-track-pattern)
          (v-stack :align :center
            (label "+"
              :font-size 12
              :color :white
              :bg :transparent)))
        (box :background "transport-btn-bg" :padding 0.2 :height 1.4
          (h-stack :gap 0.1 :align :center
            (each (range 0 (core/page-count)) |page|
              (box :width eseq.step-grid-interactions/page-button-width :height 1.1
                :background "pattern-pill-bg"
                :active (if (= page (core/visible-page)) 1 0)
                :style eseq.transport/pattern-control-style
                :on-click |x y r| (gm/goto-page page)
                (v-stack :align :center
                  (label (fmt " {} " (+ page 1))
                    :font-size 11
                    :color (if (= page (core/visible-page)) :white :dim)
                    :bg :transparent))))))))

    )))))

; Set mode after buffer exists (effect-buffer creates it above)
(set-buffer-mode-for "*metal*" "eseq.seq-grid-mode/seq-grid-mode")
