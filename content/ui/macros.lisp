;; Reusable project-macro controls for script-authored player surfaces, and
;; the *macro-mappings* table (the macro a click on a green parameter maps).
;; This file mounts no player buffer of its own. The UI manifest loads
;; ui/macro-state.lisp first.
;;
;; The macros are the host's kinds (kind-bindings spec §14.2g): project
;; macros `(macros)` (`macro`, by its id `mid`; a script names one by its
;; `script-key`), a drum rack's `rack-macro`s on its instrument device, and
;; their `macro-mapping`s (`mm.min` / `max` in the target's display units).
;;
;; This is a generated-vocabulary hub: scripts and user content call the
;; player-surface names (`macro-ensure`, `macro-knob`, `macro-momentary`,
;; `macro-map-button`, `macro-mapping-editor`, `scene-macro-controls`, …) by
;; their flat spelling through identity compat aliases
;; (tools/module-compat-aliases.tsv). Keep those names. Extension hooks are
;; addressed as data via `run-hook` (spec hazard e): a bare hook call inside a
;; module would intern a dead qualified slot instead of reaching the flat hook
;; keyspace.
(module eseq.macros)
(import eseq.kinds :refer (macros scenes tracks selection))
(import eseq.macro-state :refer (macro-arm rack-armed? arm-macro! clear-mapping-arm rack-clear-mapping-arm))
(import eseq.effects.devices :refer (instrument-of))
(import eseq.view-kit :refer (listed?))

(export macro-key-string
        macro-by-key
        macro-by-id
        macro-id-for-key
        macro-ensure
        macro-set-key-value
        macro-release-key
        macro-mapping-active-for-key?
        macro-toggle-mapping-arm
        macro-mapping-editor
        macro-mapping-table
        macro-knob
        macro-momentary
        macro-map-button
        scene-macro-controls)

;; Script keys are written in canonical lowercase. `str` preserves strings and
;; prefixes keywords with `:`, so normalize both accepted spellings for lookup.
;; The command layer performs the engine's full validation/canonicalization too.
(def macro-key-string (key)
  (let ((text (str key)))
    (if (= key text) text (substring text 1))))

;; The project macro a script names by `key`, or nil.
(def macro-by-key (key)
  (let ((script-key (macro-key-string key)))
    (first (filter (lambda (m) (= m.script-key script-key)) (macros)))))

;; The project macro with id `id`, or nil.
(def macro-by-id (id)
  (first (filter (lambda (m) (= m.mid id)) (macros))))

(def macro-id-for-key (key)
  (let ((m (macro-by-key key)))
    (if m m.mid -1)))

(def macro-ensure (key name)
  (host-command "macro-ensure"
    (dict :key (macro-key-string key) :name name)))

(def macro-set-key-value (key value)
  (let ((m (macro-by-key key)))
    (when m (set! m.value value))))

;; Releasing removes the macro's live overrides and returns it to zero.
(def release (m) (host-command "macro-release" (dict :id m.mid)))

(def macro-release-key (key)
  (let ((m (macro-by-key key)))
    (when m (release m))))

;; Whether m is the project macro armed for mapping.
(def armed? (m)
  (and macro-arm.open (= macro-arm.mid m.mid)))

(def macro-mapping-active-for-key? (key)
  (let ((m (macro-by-key key)))
    (and m (armed? m))))

(def macro-toggle-mapping-arm (key)
  (let ((m (macro-by-key key)))
    (if (= m nil)
      false
      (if (armed? m)
        (clear-mapping-arm)
        (do
          ;; Hooks are a flat keyspace and do NOT auto-qualify: reach them as
          ;; data, never as a bare call (spec hazard e).
          (run-hook "macro-mapping-arm-enter-hook")
          (run-hook "macro-mapping-sidebar-open-hook")
          (arm-macro! m.mid)
          (run-hook "macro-mapping-sidebar-refresh-hook")
          true)))))

;; The current drum rack's macro armed for mapping, or nil.
(def armed-rack-macro ()
  (let ((d (instrument-of selection.track)))
    (when (and d (rack-armed?))
      (first (filter (lambda (rm) (= rm.index macro-arm.rack-index)) d.macros)))))

;; ── The mapping rows: what a project or rack macro drives ──

;; The project macro or rack macro whose mapping mm is.
(def mapping-owner (mm) (or mm.macro mm.rack-macro))

;; The value pickers' range, decimals and unit: the target param's (its
;; display units), or the mapping's own range with 2 decimals when it drives
;; no device param (a rack slot's gain, a suspended target).
(def mapping-domain (mm)
  (let ((p mm.target))
    (if p
      (dict :lo p.min :hi p.max :unit p.unit
            :decimals (if (= p.type "continuous") (if (= p.unit "%") 1 2) 0))
      (dict :lo mm.min :hi mm.max :unit "" :decimals 2))))

(def unmap (mm)
  (let ((rm mm.rack-macro)
        (m mm.macro))
    (if rm
      (host-command "unmap-rack-macro-param"
        (dict :track rm.device.track.index :id rm.index :mapping-idx mm.index))
      ;; Neither: the mapping went (its handle is stale).
      (when m
        (host-command "macro-unmap" (dict :id m.mid :mapping-idx mm.index))))))

(def dim-label (text width size)
  (label text :width width :font-size size :color :dim :bg :transparent))

(def mapping-header ()
  (box :height 1.2 :padding 0.12 :background-color :mixer-strip-bg
    (h-stack :gap 0.25 :align :baseline
      (dim-label "Macro" 6.0 8.5)
      (dim-label "Path" 8.5 8.5)
      (dim-label "Name" 7.0 8.5)
      (dim-label "Min" 7.0 8.5)
      (dim-label "Max" 7.0 8.5)
      (dim-label "Curve" 5.0 8.5)
      (dim-label "State" 3.8 8.5)
      (dim-label "" 1.4 8.5))))

;; A range picker of mapping mm (row key `id`): `end` min or max.
(def range-picker (mm id end domain)
  (number-picker
    :key (str "macro-mapping-" end "-" id)
    :debug-name (str "macro-mapping-" end)
    :value (if (= end "min") #'mm.min #'mm.max)
    :min (get domain :lo) :max (get domain :hi)
    :decimals (get domain :decimals) :unit (get domain :unit)
    :noui true :width 7.0 :height 1.0 :font-size 8.5
    :text-align :right :text-color (if (= end "min") :dim :cyan) :edit-color :green
    :on-change (lambda (v)
      (if (= end "min") (set! mm.min v) (set! mm.max v)))))

(def mapping-row (mm)
  (let ((owner (mapping-owner mm))
        (id (str (if mm.macro owner.mid owner.index) "-" mm.index))
        (domain (mapping-domain mm)))
    (subtree :key (str "macro-mapping-row-" id)
      (box :debug-name (if mm.suspended
          "macro-mapping-table-row-suspended"
          "macro-mapping-table-row")
        :height 1.35 :padding 0.12
        :background-color (if mm.suspended
          (rgba 0.92 0.55 0.18 0.10)
          (if (and mm.macro (armed? mm.macro))
            (rgba 0.18 0.85 0.42 0.10)
            :mixer-control-bg))
        (h-stack :gap 0.25 :align :center
          (subtree :key (str "macro-mapping-name-" id)
            (label owner.name :width 6.0 :font-size 9 :color :foreground :bg :transparent :v-align :center))
          (label (substring mm.path 0 18) :width 8.5 :font-size 8.5 :color :dim :bg :transparent :v-align :center)
          (label (substring mm.param-label 0 14) :width 7.0 :font-size 8.5
            :color (if mm.suspended :dim :foreground) :v-align :center :bg :transparent)
          (range-picker mm id "min" domain)
          (range-picker mm id "max" domain)
          (dropdown
            :key (str "macro-mapping-curve-" id)
            :debug-name "macro-mapping-curve"
            :value mm.curve
            :options '("linear" "exp" "log")
            :width 5.0 :height 1.0 :font-size 8.0
            :on-change (lambda (curve) (set! mm.curve curve)))
          (label (if mm.suspended "off" "live")
            :v-align :center
            :debug-name "macro-mapping-state" :width 3.8 :font-size 8
            :color (if mm.suspended :orange :green) :bg :transparent)
          (button "×" :debug-name "macro-mapping-unmap" :width 1.4 :height 1.0 :font-size 9
            :background-color :transparent :border-color :transparent :color :dim
            :on-click (lambda (event) (unmap mm))))))))

;; Every mapping of `owners` (project or rack macros), in order.
(def mappings-of (owners)
  (reduce (lambda (all m) (append all m.mappings)) (list) owners))

(def mapping-rows (mappings)
  (v-stack :width :fill :gap 0.12
    (each mappings |mm| (mapping-row mm))))

(def mapping-empty (message)
  (box :debug-name "macro-mapping-editor-empty"
       :width :fill :height 3.2 :padding 0.6 :h-align :center :v-align :center
    (label message :width 32 :font-size 9 :h-align :center :color :dim :bg :transparent)))

;; Reusable, key-scoped editor for script-authored player surfaces.
;; Usage: (macro-mapping-editor :macro :delay-push)
(def macro-mapping-editor (_macro key)
  (let ((m (macro-by-key key)))
    (box :debug-name "macro-mapping-editor"
         :width :fill :padding 0.55 :background-color :buffer-bg
      (v-stack :width :fill :gap 0.2
        (label (str (if m m.name (macro-key-string key)) " MAPPINGS")
          :debug-name "macro-mapping-editor-title"
          :width 32 :height 1.2 :font-size 10 :color :foreground :bg :transparent)
        (if m
          (v-stack :width :fill :gap 0.2
            (mapping-header)
            (if (empty? m.mappings)
              (mapping-empty "No mappings yet — click map, then choose a green parameter")
              (mapping-rows m.mappings)))
          (mapping-empty "Macro is not available yet"))))))

;; The armed rack macro's mappings while one is armed, else every project
;; macro's.
(def macro-mapping-table ()
  (let ((rack-armed (rack-armed?))
        (rm (armed-rack-macro))
        (mappings (mappings-of (if rack-armed (if rm (list rm) (list)) (macros)))))
    (box :width :fill :height :fill :padding 0.65 :background-color :buffer-bg
      (v-stack :width :fill :gap 0.2
        (h-stack :width :fill :height 1.3 :align :center
          (label (if rack-armed "RACK MACRO MAPPINGS" "MACRO MAPPINGS")
            :width 40 :font-size 11 :color :foreground :bg :transparent)
          (button "done" :width 5.0 :height 1.05 :font-size 8.5
            :background-color (rgba 0.18 0.85 0.42 0.22) :color :foreground
            :on-click (lambda (event)
              (if rack-armed (rack-clear-mapping-arm) (clear-mapping-arm)))))
        (mapping-header)
        (if (empty? mappings)
          (mapping-empty "Click a green parameter to map it")
          (scroll :width :fill :flex 1
            (mapping-rows mappings)))))))

(set-buffer-mode-for "*macro-mappings*" "eseq.sequencer-keys/sequencer-keys")
(effect-buffer "*macro-mappings*" (macro-mapping-table))

;; ── Player-surface controls ──

;; Usage: (macro-knob :macro :delay-push)
(def macro-knob (_macro key)
  (let ((m (macro-by-key key))
        (resolved-key (macro-key-string key)))
    (subtree :key (str "macro-knob-" resolved-key)
      (knob-number
        :debug-name "macro-knob"
        :label (if m m.name resolved-key)
        :value (if m #'m.value 0)
        :min 0 :max 1 :decimals 2
        :width 7.0 :height 3.0 :knob-size 2.2
        :font-size 9.0 :label-font-size 9.0
        :label-color :dim
        :track-color '(rgba 0.4, 0.4, 0.4, 1)
        :on-change (lambda (value) (macro-set-key-value key value))))))

;; Lit while the macro is fully on.
(def held? (m) (> m.value 0.999))

;; Press and hold to drive the macro fully on; release removes its live
;; overrides and returns the visible macro position to zero.
;; Usage: (macro-momentary :macro :delay-push)
(def macro-momentary (_macro key)
  (let ((m (macro-by-key key))
        (resolved-key (macro-key-string key)))
    (subtree :key (str "macro-momentary-" resolved-key)
      (button "hold"
        :debug-name "macro-momentary"
        :width 4.8 :height 1.0 :font-size 9.0
        :disabled (= m nil)
        :active (if (and m (held? m)) 1 0)
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

;; ── Scene macros ──

(def scene-label (s) (str "Scene " (+ s.index 1)))

(def scene-labelled (text)
  (first (filter (lambda (s) (= (scene-label s) text)) (scenes))))

;; m's tracks with t in (on) or out.
(def tracks-with (m t on)
  (if on
    (if (listed? t m.tracks) m.tracks (append m.tracks (list t)))
    (filter (lambda (x) (not (= x t))) m.tracks)))

(def track-mask (m)
  (h-stack :debug-name "scene-macro-track-mask" :gap 0.3 :align :center
    (dim-label "tracks" 4 8)
    (each (tracks) |t|
      (h-stack :gap 0.1 :align :center
        (toggle :value (listed? t m.tracks)
          :on-change (lambda (on) (set! m.tracks (tracks-with m t on))))
        (dim-label (str (+ t.index 1)) 1.2 8)))))

;; Reusable scene-macro surface addressed by its stable numeric MacroId.
(def scene-macro-controls (_macro id)
  (let ((m (macro-by-id id)))
    (box :debug-name "scene-macro-controls" :width :fill :padding 0.55
         :background-color :buffer-bg
      (if m
        (v-stack :width :fill :gap 0.35
          (h-stack :width :fill :gap 0.5 :align :center
            (label (str "PUSH · SCENE " (if m.target-scene (+ m.target-scene.index 1) ""))
              :debug-name "scene-macro-title" :width 12 :font-size 10
              :color :foreground :bg :transparent)
            (dropdown :debug-name "scene-macro-target"
              :value (if m.target-scene (scene-label m.target-scene) "")
              :options (map scene-label (scenes)) :width 10.0 :height 1.0
              :on-change (lambda (text) (set! m.target-scene (scene-labelled text)))))
          (h-stack :gap 0.6 :align :center
            (knob-number :debug-name "scene-macro-knob"
              :label m.name :value #'m.value
              :min 0 :max 1 :decimals 2 :width 7.0 :height 3.0 :knob-size 2.2
              :on-change (lambda (value) (set! m.value value)))
            (subtree :key (str "scene-macro-hold-" m.mid)
              (button "hold" :debug-name "scene-macro-momentary"
                :width 4.8 :height 1.0 :active (held? m)
                :on-press (lambda (event) (set! m.value 1.0))
                :on-release (lambda (event) (release m))))
            (label (str "diff: " m.diff-count " params")
              :debug-name "scene-macro-diff" :width 10 :font-size 8
              :color :dim :bg :transparent))
          (h-stack :gap 0.45 :align :center
            (dim-label "params" 4 8)
            (toggle :debug-name "scene-macro-morph-params"
              :value m.morph-params
              :on-change (lambda (on) (set! m.morph-params on)))
            (dim-label "patterns" 5 8)
            (toggle :debug-name "scene-macro-steal-patterns"
              :value m.steal-patterns
              :on-change (lambda (on) (set! m.steal-patterns on)))
            (dropdown :debug-name "scene-macro-quantize"
              :value m.quantize :options '("off" "sixteenth" "bar")
              :width 6.5 :height 1.0
              :on-change (lambda (value) (set! m.quantize value))))
          (track-mask m))
        (label "Scene macro unavailable" :debug-name "scene-macro-missing"
          :width 14 :font-size 9 :color :dim :bg :transparent)))))
