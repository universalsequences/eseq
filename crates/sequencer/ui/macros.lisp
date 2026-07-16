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
    (host-command "macro-set-range"
      (dict :id (get macro :id)
            :mapping-idx (get mapping :mapping-idx)
            :min (if (= endpoint :min) stored (get mapping :min))
            :max (if (= endpoint :max) stored (get mapping :max))))))

(def macro-unmap-row (macro mapping)
  (host-command "macro-unmap"
    (dict :id (get macro :id) :mapping-idx (get mapping :mapping-idx))))

(def macro-mapping-table-header ()
  (box :height 1.2 :padding 0.12 :background-color :mixer-strip-bg
    (h-stack :gap 0.3 :align :center
      (label "Macro" :width 8.0 :font-size 9 :color :dim :bg :transparent)
      (label "Path" :width 10.5 :font-size 9 :color :dim :bg :transparent)
      (label "Name" :width 8.5 :font-size 9 :color :dim :bg :transparent)
      (label "Min" :width 6.3 :font-size 9 :color :dim :bg :transparent)
      (label "Max" :width 6.3 :font-size 9 :color :dim :bg :transparent)
      (label "" :width 1.6 :font-size 9 :color :dim :bg :transparent))))

(def macro-mapping-table-row (macro mapping)
  (subtree :key (str "macro-mapping-row-" (get macro :id) "-" (get mapping :mapping-idx))
    (box :debug-name "macro-mapping-table-row"
         :height 1.35 :padding 0.12
         :background-color (if (= (get macro :id) macro-mapping-selected)
           (rgba 0.18 0.85 0.42 0.10)
           :mixer-control-bg)
      (h-stack :gap 0.3 :align :center
        (label (get macro :name) :width 8.0 :font-size 9.5 :color :foreground :bg :transparent)
        (label (get mapping :path-label) :width 10.5 :font-size 9 :color :dim :bg :transparent)
        (label (get mapping :param-label) :width 8.5 :font-size 9 :color :foreground :bg :transparent)
        (number-picker
          :key (str "macro-mapping-min-" (get macro :id) "-" (get mapping :mapping-idx))
          :debug-name "macro-mapping-min"
          :value (get mapping :display-min)
          :min (get mapping :domain-min) :max (get mapping :domain-max)
          :decimals (get mapping :display-decimals) :unit (get mapping :display-unit)
          :noui true :width 6.3 :height 1.0 :font-size 9
          :text-align :right :text-color :dim :edit-color :green
          :on-change (lambda (value)
            (macro-set-mapping-display-range macro mapping :min value)))
        (number-picker
          :key (str "macro-mapping-max-" (get macro :id) "-" (get mapping :mapping-idx))
          :debug-name "macro-mapping-max"
          :value (get mapping :display-max)
          :min (get mapping :domain-min) :max (get mapping :domain-max)
          :decimals (get mapping :display-decimals) :unit (get mapping :display-unit)
          :noui true :width 6.3 :height 1.0 :font-size 9
          :text-align :right :text-color :cyan :edit-color :green
          :on-change (lambda (value)
            (macro-set-mapping-display-range macro mapping :max value)))
        (button "×" :debug-name "macro-mapping-unmap" :width 1.6 :height 1.0 :font-size 9
          :background-color :transparent :border-color :transparent :color :dim
          :on-click (lambda (event) (macro-unmap-row macro mapping)))))))

(def macro-mapping-table-row-count ()
  (reduce |count macro| (+ count (len (get macro :mappings))) 0 SEQ.macros))

(def macro-mapping-table-rows ()
  (reduce |rows macro|
    (append rows (map |mapping| (list macro mapping) (get macro :mappings)))
    '()
    SEQ.macros))

(def macro-mapping-table ()
  (box :width :fill :height :fill :padding 0.65 :background-color :buffer-bg
    (v-stack :width :fill :gap 0.2
      (h-stack :width :fill :height 1.3 :align :center
        (label "MACRO MAPPINGS" :width 35 :font-size 11 :color :foreground :bg :transparent)
        (button "done" :width 5.0 :height 1.05 :font-size 8.5
          :background-color (rgba 0.18 0.85 0.42 0.22) :color :foreground
          :on-click (lambda (event) (macro-clear-mapping-arm))))
      (macro-mapping-table-header)
      (if (= (macro-mapping-table-row-count) 0)
        (box :width :fill :height 4 :padding 0.6 :h-align :center :v-align :center
          (label "Click a green parameter to map it"
            :width 30 :font-size 9.5 :h-align :center :color :dim :bg :transparent))
        (scroll :width :fill :flex 1
          (v-stack :width :fill :gap 0.12
            (each (macro-mapping-table-rows) |row|
              (macro-mapping-table-row (nth row 0) (nth row 1)))))))))

(effect-buffer "*macro-mappings*" (macro-mapping-table))

;; Usage: (macro-knob :macro :delay-push)
(def macro-knob (_macro key)
  (let ((macro (macro-by-key key))
        (resolved-key (macro-key-string key)))
    (subtree :key (str "macro-knob-" resolved-key)
      (knob-number
        :label (if macro (get macro :name) resolved-key)
        :value (if macro (get macro :value) 0)
        :min 0 :max 1 :decimals 2
        :width 7.0 :height 3.0 :knob-size 2.2
        :font-size 9.0 :label-font-size 9.0
        :label-color :dim
        :track-color '(rgba 0.4, 0.4, 0.4, 1)
        :on-change (lambda (value) (macro-set-key-value key value))))))

;; Usage: (macro-map-button :macro :delay-push)
(def macro-map-button (_macro key)
  (let ((active (macro-mapping-active-for-key? key)))
    (button "map"
      :width 4.0 :height 1.0 :font-size 9.0
      :background-color (if active (rgba 0.27 0.78 0.43 1.0) :mixer-control-bg)
      :color (if active :black :dim)
      :on-click (lambda (event) (macro-toggle-mapping-arm key)))))
