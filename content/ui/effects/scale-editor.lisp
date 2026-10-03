;; The track scale editor (docs/microtonal-scales-spec.md §5): the settings
;; button beside the track `scale` dropdown swaps the *track* buffer to this
;; panel. Every edit goes through `seq-tuning`, one undo step per edit (a
;; drag on one degree, and the morph picker, coalesce).
(module eseq.effects.scale-editor)

(export scale-editor-open
        open-scale-editor
        close-scale-editor
        scale-settings-button
        scale-editor-panel)

(defstate scale-editor-open false)
(defstate selected-degree -1)
(defstate rand-amount 15)
(defstate stretch-amount 5)

;; The *track* tile is 28 wide with a 1-cell pad each side
;; (eseq.seq-layout/track-tile-height grows it to 13 rows while open).
(def editor-width 26.0)

;; Late-bound: eseq.seq-layout imports this module.
(def open-scale-editor ()
  (do
    (set! scale-editor-open true)
    (eseq.seq-layout/refresh-current-layout)))

(def close-scale-editor ()
  (do
    (set! scale-editor-open false)
    (set! selected-degree -1)
    (eseq.seq-layout/refresh-current-layout)))

;; The small button that sits right of the track settings `scale` dropdown.
(def scale-settings-button ()
  (button ""
    :key "track-scale-editor-open"
    :debug-name "track-scale-editor-open"
    :icon :sliders
    :variant :ghost
    :width 3.6 :height 1.0
    :on-click |x y r| (open-scale-editor)))

(def on-degree-change (kind degree value)
  (do
    (set! selected-degree degree)
    (if (= kind :set)
      (seq-tuning "offset" value degree)
      (if (= kind :clear)
        (seq-tuning "clear" 0 degree)
        (if (= kind :toggle)
          (seq-tuning "toggle" 0 degree)
          nil)))))

(def field (title body)
  (v-stack :align :center :gap 0.15
    (label title :font-size 10 :color :dim :bg :transparent :v-align :center)
    body))

(def tool-button (key text width on-click)
  (button text
    :key key
    :debug-name key
    :width width :height 1.1 :font-size 11
    :on-click |x y r| (on-click)))

(def header-row ()
  (h-stack :gap 0.3 :align :center
    (button "‹"
      :key "scale-editor-close"
      :debug-name "scale-editor-close"
      :variant :ghost
      :width 1.6 :height 1.0 :font-size 12
      :on-click |x y r| (close-scale-editor))
    (dropdown :value SEQ.tp-fts
      :key "scale-editor-scale"
      :options SEQ.fts-options
      :on-change (lambda (v) (do (set! selected-degree -1) (seq-set-fts v)))
      :width 10.6 :height 1.0 :font-size 11)
    (dropdown :value SEQ.tp-tuning-root
      :key "scale-editor-root"
      :options SEQ.tuning-root-options
      :on-change (lambda (v) (seq-tuning "root" v))
      :width 4.6 :height 1.0 :font-size 11)
    ;; Snap: nearest degree. Map: every semitone is the next degree.
    (button SEQ.tp-tuning-mode
      :key "scale-editor-mode"
      :debug-name "scale-editor-mode"
      :width 5.0 :height 1.0 :font-size 11
      :background-color (if (= SEQ.tp-tuning-mode "Map") :control-on-bg :poly-off-bg)
      :color (if (= SEQ.tp-tuning-mode "Map") :control-on-fg :poly-off-fg)
      :on-click |x y r| (seq-tuning "mode" (if (= SEQ.tp-tuning-mode "Map") "Snap" "Map")))))

(def amount-picker (key value min max on-change)
  (number-picker :value value :min min :max max :decimals 0 :unit "ct"
    :key key
    :debug-name key
    :border-color :none
    :noui false :font-size 11 :text-color :white
    :on-change on-change
    :width 4.8 :height 1.1))

;; Whole-scale actions (Reset in red, last), the two amount-driven tools,
;; then morph.
(def tools-rows ()
  (v-stack :gap 0.3
    (h-stack :gap 0.4 :align :center
      (tool-button "scale-editor-just" "Just intonation" 9.8 (lambda () (seq-tuning "just" 0)))
      (tool-button "scale-editor-import" "Load .scl" 6.6
        (lambda () (seq-tuning "import-scl" 0)))
      (button "Reset"
        :key "scale-editor-reset"
        :debug-name "scale-editor-reset"
        :variant :danger
        :width 5.0 :height 1.1 :font-size 11
        :on-click |x y r| (seq-tuning "reset" 0)))
    (h-stack :gap 0.9 :align :center
      (h-stack :gap 0.25 :align :center
        (tool-button "scale-editor-rand" "Randomize" 7.6
          (lambda () (seq-tuning "rand" rand-amount)))
        (amount-picker "scale-editor-rand-amount" rand-amount 1 100
          (lambda (v) (set! rand-amount v))))
      (h-stack :gap 0.25 :align :center
        (tool-button "scale-editor-stretch" "Stretch" 5.6
          (lambda () (seq-tuning "stretch" stretch-amount)))
        (amount-picker "scale-editor-stretch-amount" stretch-amount -50 50
          (lambda (v) (set! stretch-amount v)))))
    ;; 0% = every note on its nearest semitone, 100% = the tuning as drawn.
    (h-stack :gap 0.4 :align :center
      (label "Morph" :font-size 11 :color :dim :bg :transparent :v-align :center)
      (number-picker :value SEQ.tp-tuning-morph :min 0 :max 100 :decimals 0 :unit "%"
        :key "scale-editor-morph"
        :debug-name "scale-editor-morph"
        :border-color :none
        :noui false :font-size 11 :text-color :white
        :on-change (lambda (v) (seq-tuning "morph" (/ v 100)))
        :width 6.0 :height 1.1))))

(def off-message ()
  (box :width editor-width :height 5.0 :h-align :center :v-align :center
    :background-color '(rgba 0.0 0.0 0.0 0.22) :corner-radius 4
    (label "pick a scale to tune it" :font-size 11 :color :dim :bg :transparent)))

(def scale-editor-panel ()
  (box :debug-name "scale-editor-panel" :padding 0.0
    (v-stack :gap 0.3
      (header-row)
      (if SEQ.tp-tuning-on
        (scale-editor
          :key "scale-editor-degrees"
          :debug-name "scale-editor-degrees"
          :base SEQ.tp-tuning-base
          :offsets SEQ.tp-tuning-offsets
          :pitches SEQ.tp-tuning-pitches
          :enabled SEQ.tp-tuning-enabled
          :labels SEQ.tp-tuning-labels
          :period SEQ.tp-tuning-period
          :selected selected-degree
          :range 100
          :font-size 9
          :width editor-width :height 5.0
          :on-change (lambda (kind degree value) (on-degree-change kind degree value)))
        (off-message))
      (tools-rows))))
