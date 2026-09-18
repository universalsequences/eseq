;; ui/customize.lisp -- the Customize modal: an Emacs-customize-style
;; surface over every `defcustom` knob, grouped by the module that declared
;; it, plus an Overrides section that lists every `override` registration by
;; the package that installed it with enable/disable toggles.
;;
;; Editors call `setopt-by-name`, so changes are live; Reset restores the
;; declared default. Nothing here writes to disk: Save asks the host to
;; rewrite the managed block in ~/.eseq.d/init.lisp
;; (`save-custom-values`), and a knob back on its default drops out of it.
;;
;; Library module: `panel` is mounted by eseq.file-dialogs next to the other
;; dialogs, and `customize` (M-x) asks the host to activate that tile first,
;; because a modal only receives pointer input through the active tile.
(module eseq.customize)

(export customize
        open-customize
        close-customize
        customize-open?
        customize-save
        panel
        customize-dirty
        customize-generation
        reset-knob
        set-knob
        set-override-enabled
        set-module-overrides-enabled
        module-overrides-enabled?)

;; ── State ──

(defstate customize-open? false)
;; Unsaved edits since the last Save (or since the buffer opened).
(defstate customize-dirty false)
;; `override-declarations` is a registry read, not a reactive source; bump
;; this after a toggle so the buffer re-reads it.
(defstate customize-generation 0)

;; ── Helpers ──

(def member? (xs x)
  (reduce (lambda (acc y) (if (= y x) true acc)) false xs))

;; Distinct :module values in first-seen order (rows arrive sorted).
(def modules-of (rows)
  (reduce
    (lambda (acc row)
      (if (member? acc (get row :module)) acc (append acc (list (get row :module)))))
    (list)
    rows))

(def rows-in-module (rows module)
  (filter (lambda (row) (= (get row :module) module)) rows))

;; "eseq.mixer/strip-width" -> "strip-width"
(def short-name (name module)
  (if (= module "")
    name
    (substring name (+ (len module) 1))))

(def touch ()
  (do
    (set! customize-dirty true)
    (set! customize-generation (+ customize-generation 1))))

;; ── Knobs ──

(def set-knob (name value)
  (do
    (setopt-by-name name value)
    (set! customize-dirty true)))

(def reset-knob (row)
  (set-knob (get row :name) (get row :default)))

;; Range for a number knob: declared `:min`/`:max`/`:step`, else derived
;; from the default so a picker drag moves in sensible steps. A default of 4
;; gets 0..16, 0.2 gets 0..1, 12.9 gets 0..52; negatives mirror. Ratios
;; (defaults below 1) step by hundredths, everything else by tenths.
(def knob-min (row)
  (let ((declared (get row :min)) (d (get row :default)))
    (if declared declared (if (< d 0) (* d 4) 0))))

(def knob-max (row)
  (let ((declared (get row :max)) (d (get row :default)))
    (if declared declared
      (if (<= d 0) (max 10 (* (abs d) 4))
        (if (< d 1) 1 (max 10 (* d 4)))))))

(def knob-step (row)
  (let ((declared (get row :step)) (d (get row :default)))
    (if declared declared (if (and (> d 0) (< d 1)) 0.01 0.1))))

(def knob-decimals (row)
  (if (< (knob-step row) 1) 2 0))

(def knob-editor (row)
  (let ((name (get row :name))
        (value (get row :value))
        (type (get row :type))
        (choices (get row :choices))
        (key (str "customize-" name)))
    (if (> (len choices) 0)
      (let ((options (map choices |choice| (str choice))))
        (dropdown :key key :width 12 :height 1.2 :font-size 10
          :value (str value) :options options
          :on-change (lambda (picked)
            (each (range 0 (len choices)) |index|
              (if (= picked (nth options index))
                (set-knob name (nth choices index))
                nil)))))
      (if (= type :bool)
        (toggle :key key :value value
          :on-change (lambda (v) (set-knob name v)))
        (if (= type :number)
          (number-picker :key key :width 8 :height 1.2 :font-size 10
            :value value :min (knob-min row) :max (knob-max row)
            :step (knob-step row) :decimals (knob-decimals row)
            :on-change (lambda (v) (set-knob name v)))
          (text-input :key key :width 14 :height 1.2 :font-size 10
            :value (str value)
            :on-change |v| (set-knob name v)))))))

(def knob-row (row)
  (let ((name (get row :name))
        (changed (not (= (get row :value) (get row :default)))))
    (h-stack :key (str "customize-row-" name) :width :fill :gap 0.6 :v-align :center
      (v-stack :flex 1 :gap 0.1
        (h-stack :gap 0.4 :v-align :center
          (label (short-name name (get row :module))
            :font-size 11 :color (if changed :accent :white) :bg :transparent)
          (if changed
            (label "(customized)" :font-size 8 :color :dim :bg :transparent)
            nil))
        (label (get row :doc) :font-size 8.5 :color :dim :bg :transparent))
      (knob-editor row)
      (button "Reset" :key (str "customize-reset-" name) :variant :ghost
        :disabled (not changed)
        :on-click |x y r| (reset-knob row)))))

(def module-section (module rows)
  (v-stack :key (str "customize-module-" module) :width :fill :gap 0.35
    (label module :font-size 12 :color :accent :bg :transparent)
    (each (rows-in-module rows module) |row| (knob-row row))))

(def knobs-section (rows)
  (if (= (len rows) 0)
    (label "No customizable knobs are declared." :font-size 10 :color :dim :bg :transparent)
    (v-stack :width :fill :gap 0.9
      (each (modules-of rows) |module| (module-section module rows)))))

;; ── Overrides ──

(def set-override-enabled (row enabled)
  (do
    (set-override-entry-enabled (get row :target) (get row :module) enabled)
    (touch)))

(def module-overrides-enabled? (rows module)
  (reduce (lambda (acc row) (if (get row :enabled) true acc))
    false
    (rows-in-module rows module)))

;; Module-level switches also record the module in the persisted disabled
;; list, so a package the user turned off stays off after a reload.
(def set-module-overrides-enabled (module enabled)
  (do
    (__set-module-overrides-enabled module enabled)
    (touch)))

(def override-row (row)
  (let ((target (get row :target))
        (module (get row :module))
        (enabled (get row :enabled)))
    (h-stack :key (str "customize-override-" module "-" target)
      :width :fill :gap 0.6 :v-align :center
      (toggle :key (str "customize-override-toggle-" module "-" target)
        :value enabled
        :on-change (lambda (v) (set-override-enabled row v)))
      (label target :font-size 10.5 :color (if enabled :white :dim) :bg :transparent :flex 1)
      (label (if (= (get row :kind) :around) "around" "replace")
        :font-size 8.5 :color :dim :bg :transparent)
      (if (get row :quarantined)
        (label "quarantined: errored, factory in use" :font-size 8.5 :color :accent :bg :transparent)
        nil))))

(def override-module-section (module rows)
  (let ((enabled (module-overrides-enabled? rows module)))
    (v-stack :key (str "customize-override-module-" module) :width :fill :gap 0.35
      (h-stack :width :fill :gap 0.6 :v-align :center
        (toggle :key (str "customize-override-module-toggle-" module)
          :value enabled
          :on-change (lambda (v) (set-module-overrides-enabled module v)))
        (label module :font-size 12 :color :accent :bg :transparent :flex 1)
        (label (if enabled "on" "off (factory)") :font-size 9 :color :dim :bg :transparent))
      (each (rows-in-module rows module) |row| (override-row row)))))

(def overrides-section (rows)
  (v-stack :width :fill :gap 0.9
    (label "Overrides" :font-size 14 :color :white :bg :transparent)
    (label "Advice installed on factory definitions, by the module that installed it. Turning a module off returns its targets to the factory definitions without unloading it."
      :font-size 8.5 :color :dim :bg :transparent)
    (if (= (len rows) 0)
      (label "No overrides are registered." :font-size 10 :color :dim :bg :transparent)
      (each (modules-of rows) |module| (override-module-section module rows)))))

;; ── Root ──

(def header ()
  (h-stack :width :fill :gap 0.8 :v-align :center
    (label "Customize" :font-size 16 :color :white :bg :transparent)
    (label (if customize-dirty "unsaved changes" "saved") :font-size 9
      :color (if customize-dirty :accent :dim) :bg :transparent :flex 1)
    (button "Save" :key "customize-save" :variant :primary
      :disabled (not customize-dirty)
      :on-click |x y r| (customize-save))
    (button "Close" :key "customize-close" :on-click |x y r| (close-customize))))

(def root-widget ()
  (let ((generation customize-generation)
        (knobs (custom-declarations))
        (overrides (override-declarations)))
    (v-stack :width :fill :height :fill :gap 0.6 :padding 0.8
      (header)
      (scroll :key "customize-scroll" :width :fill :flex 1
        (box :width :fill :padding-right 1.0
          (v-stack :width :fill :gap 1.4
            (v-stack :width :fill :gap 0.9
              (label "Knobs" :font-size 14 :color :white :bg :transparent)
              (knobs-section knobs))
            (overrides-section overrides)))))))

(def panel ()
  (modal :is-open customize-open? :on-close (lambda () (close-customize))
      :width-px 1400 :height-px 900
    (box :debug-name "customize-panel" :width :fill :height :fill :padding 0.6 :bg :transparent
      (if customize-open? (root-widget) (box :width 0 :height 0 :bg :transparent)))))

;; ── Commands ──

(def open-customize ()
  (do
    (set! customize-generation (+ customize-generation 1))
    (set! customize-open? true)))

(def close-customize ()
  (set! customize-open? false))

;; M-x entry point: the host activates the tile that mounts `panel`, then
;; opens the modal.
(def customize ()
  (host-command "customize-open" (dict)))

(def customize-save ()
  (host-command "save-custom-values" (dict)))
