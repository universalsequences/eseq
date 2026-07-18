;; Reusable project-macro controls for script-authored player surfaces.
;; This file intentionally mounts no buffer of its own. The UI manifest loads
;; ui/macro-state.lisp first.

;; Script keys are written in canonical lowercase. `str` preserves strings and
;; prefixes keywords with `:`, so normalize both accepted spellings for lookup.
;; The command layer performs the engine's full validation/canonicalization too.
(def macro-key-string (key)
  (let ((text (str key)))
    (if (= key text) text (substring text 1))))

(def macro-by-key (key)
  (nth
    (filter |macro| (= (get macro :key) (macro-key-string key)) SEQ.macros)
    0))

(def macro-by-id (id)
  (nth (filter |macro| (= (get macro :id) id) SEQ.macros) 0))

(def macro-id-for-key (key)
  (let ((macro (macro-by-key key)))
    (if macro (get macro :id) -1)))

(def macro-ensure (key name)
  (host-command "macro-ensure"
    (dict :key (macro-key-string key) :name name)))

(def macro-set-key-value (key value)
  (let ((id (macro-id-for-key key)))
    (if (>= id 0)
      (host-command "macro-set-value" (dict :id id :value value))
      false)))

(def macro-release-key (key)
  (let ((id (macro-id-for-key key)))
    (if (>= id 0)
      (host-command "macro-release" (dict :id id))
      false)))

(def macro-mapping-active-for-key? (key)
  (let ((id (macro-id-for-key key)))
    (and macro-mapping-open (>= id 0) (= macro-mapping-selected id))))

(def macro-toggle-mapping-arm (key)
  (let ((id (macro-id-for-key key)))
    (if (< id 0)
      false
      (if (macro-mapping-active-for-key? key)
        (macro-clear-mapping-arm)
        (do
          (macro-mapping-arm-enter-hook)
          (macro-mapping-sidebar-open-hook)
          (set! macro-mapping-open true)
          (set! macro-mapping-selected id)
          (macro-mapping-sidebar-refresh-hook))))))

(def macro-set-mapping-display-range (macro mapping endpoint value)
  (let ((scale (get mapping :display-scale))
        (stored (/ value (if scale scale 1.0))))
    (host-command
      (if (= (get macro :scope) "rack") "set-rack-macro-range" "macro-set-range")
      (dict :track SEQ.current-track
            :id (get macro :id)
            :mapping-idx (get mapping :mapping-idx)
            :min (if (= endpoint :min) stored (get mapping :min))
            :max (if (= endpoint :max) stored (get mapping :max))))))

(def macro-set-mapping-curve (macro mapping curve)
  (host-command (if (= (get macro :scope) "rack") "set-rack-macro-curve" "macro-set-curve")
    (dict :track SEQ.current-track :id (get macro :id)
          :mapping-idx (get mapping :mapping-idx)
          :curve curve)))

(def macro-unmap-row (macro mapping)
  (host-command (if (= (get macro :scope) "rack") "unmap-rack-macro-param" "macro-unmap")
    (dict :track SEQ.current-track :id (get macro :id)
      :mapping-idx (get mapping :mapping-idx))))

(def rack-macro-mapping-table-macro ()
  (let ((panel (nth SEQ.instrument-panel 0)))
    (if panel
      (nth (filter |macro| (= (get macro :id) rack-macro-mapping-selected)
        (get panel :macros)) 0)
      false)))

(def active-macro-mapping-table-macros ()
  (if (>= rack-macro-mapping-selected 0)
    (let ((macro (rack-macro-mapping-table-macro)))
      (if macro (list macro) '()))
    SEQ.macros))

(def macro-mapping-editor-header ()
  (box :height 1.2 :padding 0.12 :background-color :mixer-strip-bg
    (h-stack :gap 0.25 :align :baseline
      (label "Macro" :width 6.0 :font-size 8.5 :color :dim :bg :transparent)
      (label "Path" :width 16.5 :font-size 8.5 :color :dim :bg :transparent)
      (label "Name" :width 10.0 :font-size 8.5 :color :dim :bg :transparent)
      (label "Min" :width 5.0 :font-size 8.5 :color :dim :bg :transparent)
      (label "Max" :width 5.0 :font-size 8.5 :color :dim :bg :transparent)
      (label "Curve" :width 5.0 :font-size 8.5 :color :dim :bg :transparent)
      (label "State" :width 3.8 :font-size 8.5 :color :dim :bg :transparent)
      (label "" :width 1.4 :font-size 8.5 :color :dim :bg :transparent))))

(def macro-mapping-editor-row (macro mapping)
  (subtree :key (str "macro-mapping-row-" (get macro :id) "-" (get mapping :mapping-idx))
    (box :debug-name (if (get mapping :suspended)
           "macro-mapping-table-row-suspended"
           "macro-mapping-table-row")
         :height 1.35 :padding 0.12
         :background-color (if (get mapping :suspended)
           (rgba 0.92 0.55 0.18 0.10)
           (if (= (get macro :id) macro-mapping-selected)
             (rgba 0.18 0.85 0.42 0.10)
             :mixer-control-bg))
      (h-stack :gap 0.25 :align :baseline
        (label (get macro :name) :width 6.0 :font-size 9 :color :foreground :bg :transparent)
        (label (get mapping :path-label) :width 16.5 :font-size 8.5 :color :dim :bg :transparent)
        (label (get mapping :param-label) :width 10.0 :font-size 8.5
          :color (if (get mapping :suspended) :dim :foreground) :bg :transparent)
        (number-picker
          :key (str "macro-mapping-min-" (get macro :id) "-" (get mapping :mapping-idx))
          :debug-name "macro-mapping-min"
          :value (get mapping :display-min)
          :min (get mapping :domain-min) :max (get mapping :domain-max)
          :decimals (get mapping :display-decimals) :unit (get mapping :display-unit)
          :noui true :width 5.0 :height 1.0 :font-size 8.5
          :text-align :right :text-color :dim :edit-color :green
          :on-change (lambda (value)
            (macro-set-mapping-display-range macro mapping :min value)))
        (number-picker
          :key (str "macro-mapping-max-" (get macro :id) "-" (get mapping :mapping-idx))
          :debug-name "macro-mapping-max"
          :value (get mapping :display-max)
          :min (get mapping :domain-min) :max (get mapping :domain-max)
          :decimals (get mapping :display-decimals) :unit (get mapping :display-unit)
          :noui true :width 5.0 :height 1.0 :font-size 8.5
          :text-align :right :text-color :cyan :edit-color :green
          :on-change (lambda (value)
            (macro-set-mapping-display-range macro mapping :max value)))
        (dropdown
          :key (str "macro-mapping-curve-" (get macro :id) "-" (get mapping :mapping-idx))
          :debug-name "macro-mapping-curve"
          :value (get mapping :curve)
          :options '("linear" "exp" "log")
          :width 5.0 :height 1.0 :font-size 8.0
          :on-change (lambda (curve) (macro-set-mapping-curve macro mapping curve)))
        (label (if (get mapping :suspended) "off" "live")
          :debug-name "macro-mapping-state" :width 3.8 :font-size 8
          :color (if (get mapping :suspended) :orange :green) :bg :transparent)
        (button "×" :debug-name "macro-mapping-unmap" :width 1.4 :height 1.0 :font-size 9
          :background-color :transparent :border-color :transparent :color :dim
          :on-click (lambda (event) (macro-unmap-row macro mapping)))))))

(def macro-mapping-editor-row-count (macros)
  (reduce |count macro| (+ count (len (get macro :mappings))) 0 macros))

(def macro-mapping-editor-rows-for-macro (macro)
  (reduce |rows mapping|
    (append rows (list (list macro mapping)))
    '()
    (get macro :mappings)))

(def macro-mapping-editor-rows (macros)
  (reduce |rows macro|
    (append rows (macro-mapping-editor-rows-for-macro macro))
    '()
    macros))

(def macro-mapping-editor-row-list (macros)
  (v-stack :width :fill :gap 0.12
    (each (macro-mapping-editor-rows macros) |row|
      (macro-mapping-editor-row (nth row 0) (nth row 1)))))

(def macro-mapping-editor-empty (message)
  (box :debug-name "macro-mapping-editor-empty"
       :width :fill :height 3.2 :padding 0.6 :h-align :center :v-align :center
    (label message :width 32 :font-size 9 :h-align :center :color :dim :bg :transparent)))

;; Reusable, key-scoped editor for script-authored player surfaces.
;; Usage: (macro-mapping-editor :macro :delay-push)
(def macro-mapping-editor (_macro key)
  (let ((macro (macro-by-key key))
        (resolved-key (macro-key-string key)))
    (box :debug-name "macro-mapping-editor"
         :width :fill :padding 0.55 :background-color :buffer-bg
      (v-stack :width :fill :gap 0.2
        (label (if macro (str (get macro :name) " MAPPINGS") (str resolved-key " MAPPINGS"))
          :debug-name "macro-mapping-editor-title"
          :width 32 :height 1.2 :font-size 10 :color :foreground :bg :transparent)
        (if macro
          (v-stack :width :fill :gap 0.2
            (macro-mapping-editor-header)
            (if (= (len (get macro :mappings)) 0)
              (macro-mapping-editor-empty "No mappings yet — click map, then choose a green parameter")
              (macro-mapping-editor-row-list (list macro))))
          (macro-mapping-editor-empty "Macro is not available yet"))))))

(def macro-mapping-table ()
  (let ((macros (active-macro-mapping-table-macros))
        (rack-active (>= rack-macro-mapping-selected 0)))
    (box :width :fill :height :fill :padding 0.65 :background-color :buffer-bg
      (v-stack :width :fill :gap 0.2
        (h-stack :width :fill :height 1.3 :align :center
          (label (if rack-active "RACK MACRO MAPPINGS" "MACRO MAPPINGS")
            :width 35 :font-size 11 :color :foreground :bg :transparent)
          (button "done" :width 5.0 :height 1.05 :font-size 8.5
            :background-color (rgba 0.18 0.85 0.42 0.22) :color :foreground
            :on-click (lambda (event)
              (if rack-active (rack-macro-clear-mapping-arm) (macro-clear-mapping-arm)))))
        (macro-mapping-editor-header)
        (if (= (macro-mapping-editor-row-count macros) 0)
          (macro-mapping-editor-empty "Click a green parameter to map it")
          (scroll :width :fill :flex 1
            (macro-mapping-editor-row-list macros)))))))

(effect-buffer "*macro-mappings*" (macro-mapping-table))

;; Usage: (macro-knob :macro :delay-push)
(def macro-knob (_macro key)
  (let ((macro (macro-by-key key))
        (resolved-key (macro-key-string key)))
    (subtree :key (str "macro-knob-" resolved-key)
      (knob-number
        :debug-name "macro-knob"
        :label (if macro (get macro :name) resolved-key)
        :value (if macro (get macro :value) 0)
        :min 0 :max 1 :decimals 2
        :width 7.0 :height 3.0 :knob-size 2.2
        :font-size 9.0 :label-font-size 9.0
        :label-color :dim
        :track-color '(rgba 0.4, 0.4, 0.4, 1)
        :on-change (lambda (value) (macro-set-key-value key value))))))

;; Press and hold to drive the macro fully on; release removes its live
;; overrides and returns the visible macro position to zero.
;; Usage: (macro-momentary :macro :delay-push)
(def macro-momentary (_macro key)
  (let ((macro (macro-by-key key))
        (resolved-key (macro-key-string key)))
    (subtree :key (str "macro-momentary-" resolved-key)
      (button "hold"
        :debug-name "macro-momentary"
        :width 4.8 :height 1.0 :font-size 9.0
        :disabled (if macro false true)
        :active (if (and macro (> (get macro :value) 0.999)) 1 0)
        :background-color :mixer-control-bg
        :active-background-color (rgba 0.27 0.78 0.43 1.0)
        :color :dim :active-color :black
        :on-press (lambda (event) (macro-set-key-value key 1.0))
        :on-release (lambda (event) (macro-release-key key))))))

;; Usage: (macro-map-button :macro :delay-push)
(def macro-map-button (_macro key)
  (let ((active (macro-mapping-active-for-key? key)))
    (button "map"
      :debug-name "macro-map-button"
      :width 4.0 :height 1.0 :font-size 9.0
      :background-color (if active (rgba 0.27 0.78 0.43 1.0) :mixer-control-bg)
      :color (if active :black :dim)
      :on-click (lambda (event) (macro-toggle-mapping-arm key)))))

(def scene-macro-options ()
  (map |scene| (str "Scene " (+ scene 1)) (range 0 SEQ.num-patterns)))

(def scene-macro-option-index (label)
  (reduce |found scene|
    (if (= label (str "Scene " (+ scene 1))) scene found)
    0
    (range 0 SEQ.num-patterns)))

(def scene-macro-track-mask-with (macro selected value)
  (let ((mask (get macro :track-mask)))
    (map |track|
      (if (= track selected)
        value
        (if mask (nth mask track) true))
      (range 0 (len SEQ.track-names)))))

(def scene-macro-config (macro fields)
  (host-command "macro-scene-config" (merge (dict :id (get macro :id)) fields)))

;; Reusable scene-macro surface addressed by its stable numeric MacroId.
(def scene-macro-controls (_macro id)
  (let ((macro (macro-by-id id)))
    (box :debug-name "scene-macro-controls" :width :fill :padding 0.55
         :background-color :buffer-bg
      (if macro
        (v-stack :width :fill :gap 0.35
          (h-stack :width :fill :gap 0.5 :align :center
            (label (str "PUSH · SCENE " (+ (get macro :target-scene) 1))
              :debug-name "scene-macro-title" :width 12 :font-size 10
              :color :foreground :bg :transparent)
            (dropdown :debug-name "scene-macro-target"
              :value (nth (scene-macro-options) (get macro :target-scene))
              :options (scene-macro-options) :width 10.0 :height 1.0
              :on-change (lambda (label)
                (scene-macro-config macro (dict :target-scene
                  (scene-macro-option-index label))))))
          (h-stack :gap 0.6 :align :center
            (knob-number :debug-name "scene-macro-knob"
              :label (get macro :name) :value (get macro :value)
              :min 0 :max 1 :decimals 2 :width 7.0 :height 3.0 :knob-size 2.2
              :on-change (lambda (value)
                (host-command "macro-set-value" (dict :id (get macro :id) :value value))))
            (button "hold" :debug-name "scene-macro-momentary"
              :width 4.8 :height 1.0 :active (> (get macro :value) 0.999)
              :on-press (lambda (event)
                (host-command "macro-set-value" (dict :id (get macro :id) :value 1.0)))
              :on-release (lambda (event)
                (host-command "macro-release" (dict :id (get macro :id)))))
            (label (str "diff: " (get macro :diff-count) " params")
              :debug-name "scene-macro-diff" :width 10 :font-size 8
              :color :dim :bg :transparent))
          (h-stack :gap 0.45 :align :center
            (label "params" :width 4 :font-size 8 :color :dim :bg :transparent)
            (toggle :debug-name "scene-macro-morph-params"
              :value (get macro :morph-params)
              :on-change (lambda (value)
                (scene-macro-config macro (dict :morph-params value))))
            (label "patterns" :width 5 :font-size 8 :color :dim :bg :transparent)
            (toggle :debug-name "scene-macro-steal-patterns"
              :value (get macro :steal-patterns)
              :on-change (lambda (value)
                (scene-macro-config macro (dict :steal-patterns value))))
            (dropdown :debug-name "scene-macro-quantize"
              :value (get macro :quantize) :options '("off" "sixteenth" "bar")
              :width 6.5 :height 1.0
              :on-change (lambda (value)
                (scene-macro-config macro (dict :quantize value)))))
          (h-stack :debug-name "scene-macro-track-mask" :gap 0.3 :align :center
            (label "tracks" :width 4 :font-size 8 :color :dim :bg :transparent)
            (each (range 0 (len SEQ.track-names)) |track|
              (h-stack :gap 0.1 :align :center
                (toggle :value (if (get macro :track-mask)
                                  (nth (get macro :track-mask) track) true)
                  :on-change (lambda (value)
                    (scene-macro-config macro (dict :track-mask
                      (scene-macro-track-mask-with macro track value)))))
                (label (str (+ track 1)) :width 1.2 :font-size 8
                  :color :dim :bg :transparent)))))
        (label "Scene macro unavailable" :debug-name "scene-macro-missing"
          :width 14 :font-size 9 :color :dim :bg :transparent)))))
