;; The track scale editor (docs/microtonal-scales-spec.md §5): the settings
;; button beside the track `scale` dropdown swaps the *track* buffer to this
;; panel. It edits the current track's `tuning` (eseq.kinds): every edit is
;; one undo step (a drag on one degree, and the morph picker, coalesce).
(module eseq.effects.scale-editor)

(import eseq.kinds :refer (selection project tuning-root-options reset-tuning!
                           justify-tuning! randomize-tuning! stretch-tuning!
                           clear-degree!))

(export scale-view
        editor-open?
        open-scale-editor
        close-scale-editor
        scale-settings-button
        scale-editor-panel)

;; Whether the editor replaces the track settings, the degree last edited
;; (-1 none) and the Randomize / Stretch amounts in cents.
(def-kind scale-view
  :key ()
  :state ((open false)
          (selected-degree -1)
          (rand-amount 15)
          (stretch-amount 5)))

;; Whether the editor shows (the track panel switches on it;
;; eseq.seq-layout sizes the *track* tile by it, re-laying out on open and
;; close).
(def editor-open? () scale-view.open)

;; The *track* tile is 28 wide with a 1-cell pad each side
;; (eseq.seq-layout/track-tile-height grows it to 13 rows while open).
(def editor-width 26.0)

;; Late-bound: eseq.seq-layout imports this module.
(def open-scale-editor ()
  (set! scale-view.open true)
  (eseq.seq-layout/refresh-current-layout))

(def close-scale-editor ()
  (set! scale-view.open false)
  (set! scale-view.selected-degree -1)
  (eseq.seq-layout/refresh-current-layout))

;; The small button that sits right of the track settings `scale` dropdown.
(def scale-settings-button ()
  (button ""
    :key "track-scale-editor-open"
    :debug-name "track-scale-editor-open"
    :icon :sliders
    :variant :ghost
    :width 3.6 :height 1.0
    :on-click |x y r| (open-scale-editor)))

(def on-degree-change (tn kind index value)
  (set! scale-view.selected-degree index)
  (let ((dg (nth tn.degrees index)))
    (when dg
      (match kind
        :set (set! dg.offset value)
        :clear (clear-degree! dg)
        :toggle (toggle! dg.enabled)
        _ nil))))

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

(def header-row (t tn)
  (h-stack :gap 0.3 :align :center
    (button "‹"
      :key "scale-editor-close"
      :debug-name "scale-editor-close"
      :variant :ghost
      :width 1.6 :height 1.0 :font-size 12
      :on-click |x y r| (close-scale-editor))
    (dropdown :value t.fts
      :key "scale-editor-scale"
      :options project.fts-options
      :on-change (lambda (v) (do (set! scale-view.selected-degree -1) (seq-set-fts v)))
      :width 10.6 :height 1.0 :font-size 11)
    (dropdown :value tn.root
      :key "scale-editor-root"
      :options tuning-root-options
      :on-change (lambda (v) (set! tn.root v))
      :width 4.6 :height 1.0 :font-size 11)
    ;; Snap: nearest degree. Map: every semitone is the next degree.
    (let ((map-mode (= tn.mode "Map")))
      (button tn.mode
        :key "scale-editor-mode"
        :debug-name "scale-editor-mode"
        :width 5.0 :height 1.0 :font-size 11
        :background-color (if map-mode :control-on-bg :poly-off-bg)
        :color (if map-mode :control-on-fg :poly-off-fg)
        :on-click |x y r| (set! tn.mode (if map-mode "Snap" "Map"))))))

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
(def tools-rows (tn)
  (v-stack :gap 0.3
    (h-stack :gap 0.4 :align :center
      (tool-button "scale-editor-just" "Just intonation" 9.8 (lambda () (justify-tuning! tn)))
      (tool-button "scale-editor-import" "Load .scl" 6.6
        (lambda () (seq-tuning "import-scl" 0)))
      (button "Reset"
        :key "scale-editor-reset"
        :debug-name "scale-editor-reset"
        :variant :danger
        :width 5.0 :height 1.1 :font-size 11
        :on-click |x y r| (reset-tuning! tn)))
    (h-stack :gap 0.9 :align :center
      (h-stack :gap 0.25 :align :center
        (tool-button "scale-editor-rand" "Randomize" 7.6
          (lambda () (randomize-tuning! tn scale-view.rand-amount)))
        (amount-picker "scale-editor-rand-amount" #'scale-view.rand-amount 1 100
          (lambda (v) (set! scale-view.rand-amount v))))
      (h-stack :gap 0.25 :align :center
        (tool-button "scale-editor-stretch" "Stretch" 5.6
          (lambda () (stretch-tuning! tn scale-view.stretch-amount)))
        (amount-picker "scale-editor-stretch-amount" #'scale-view.stretch-amount -50 50
          (lambda (v) (set! scale-view.stretch-amount v)))))
    ;; 0% = every note on its nearest semitone, 100% = the tuning as drawn.
    (h-stack :gap 0.4 :align :center
      (label "Morph" :font-size 11 :color :dim :bg :transparent :v-align :center)
      (number-picker :value (round (* tn.morph 100)) :min 0 :max 100 :decimals 0 :unit "%"
        :key "scale-editor-morph"
        :debug-name "scale-editor-morph"
        :border-color :none
        :noui false :font-size 11 :text-color :white
        :on-change (lambda (v) (set! tn.morph (/ v 100)))
        :width 6.0 :height 1.1))))

(def off-message ()
  (box :width editor-width :height 5.0 :h-align :center :v-align :center
    :background-color '(rgba 0.0 0.0 0.0 0.22) :corner-radius 4
    (label "pick a scale to tune it" :font-size 11 :color :dim :bg :transparent)))

;; The degrees in their own subtree: picking a degree re-renders it alone
;; (the widget's :selected takes no binding).
(def degrees-editor (tn)
  (subtree :key "scale-editor-degrees-subtree"
    (let ((degrees tn.degrees))
      (scale-editor
        :key "scale-editor-degrees"
        :debug-name "scale-editor-degrees"
        :base (map (lambda (dg) dg.base) degrees)
        :offsets (map (lambda (dg) dg.offset) degrees)
        :pitches (map (lambda (dg) dg.pitch) degrees)
        :enabled (map (lambda (dg) dg.enabled) degrees)
        :labels (map (lambda (dg) dg.label) degrees)
        :period tn.period
        :selected scale-view.selected-degree
        :range 100
        :font-size 9
        :width editor-width :height 5.0
        :on-change (lambda (kind index value) (on-degree-change tn kind index value))))))

(def scale-editor-panel ()
  (let ((t selection.track)
        (tn (if t t.tuning nil)))
    (if tn
      (box :debug-name "scale-editor-panel" :padding 0.0
        (v-stack :gap 0.3
          (header-row t tn)
          (if tn.on (degrees-editor tn) (off-message))
          (tools-rows tn)))
      (box :width 0 :height 0))))
