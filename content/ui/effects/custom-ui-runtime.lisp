;; Runtime binding helpers exposed to generated custom instrument and effect UIs.
(module eseq.effects.custom-ui-runtime)

(import eseq.effects.param-controls :as pc)
(import eseq.effects.param-grid :as pg)
(import eseq.effects.custom-ui-sections :as sec)
;; runtime <-> custom-effect-ui is a converted-module import cycle
;; (the sections <-> runtime precedent); load-once terminates it.
(import eseq.effects.custom-effect-ui :as fxui)

(export inst-param
        inst-base-note-param
        ui-param-control
        custom-ui-scope-name
        custom-ui-current-scope
        custom-ui-param-in-scope
        custom-ui-set-param-in-scope
        custom-ui-set-param-by-name-in-scope
        custom-ui-set-adsr-in-scope
        custom-ui-set-envelope-in-scope
        custom-ui-param-change-callback
        custom-ui-param-change-callback-s
        custom-ui-xy-change-callback-s
        custom-ui-current-param
        custom-ui-current-tensor-param
        custom-ui-current-base-note-param
        custom-ui-set-param
        custom-ui-param-binding
        custom-ui-param-value
        custom-ui-param-control-min
        custom-ui-param-control-max
        custom-ui-param-control-unit
        custom-ui-param-mod-wrapper
        custom-ui-param-control-key-mode
        custom-ui-param-base-value-prop
        custom-ui-param-mod-offset
        custom-ui-param-mod-scale
        custom-ui-param-process-value
        custom-ui-param-process-clamped
        custom-ui-param-process-mapped?
        custom-ui-param-base-min-prop
        custom-ui-param-base-max-prop
        custom-ui-param-plock-active?
        custom-ui-param-plock-default
        custom-ui-param-plock-text-color
        custom-ui-param-mod-highlighted?
        custom-ui-param-knob-mod-slot-prop
        custom-ui-param-knob-mod-depth-prop
        custom-ui-selected-mod-slot-prop
        custom-ui-tensor-bound-values
        custom-ui-tensor-cell-change-callback
        custom-ui-tensor-cell-change-callback-s
        base-note)

;; Migration aliases (module spec §10). Every alias is an identity alias:
;; this file is the generated-custom-UI vocabulary (hub-file precedent —
;; the flat spellings are the contract generated code speaks), and its
;; callers are the unconverted custom-UI family (custom-ui-controls /
;; custom-ui-lego / custom-effect-ui / panel-bodies), generated
;; per-instrument ui.lisp files (content/instruments/**/ui.lisp),
;; Rust-generated lisp (src/ui/custom_ui.rs emits `(ui-param-control …)`
;; calls into implicit-module units), the agent-validated vocabulary
;; (src/agent/ui_validate.rs arity table names custom-ui-param-in-scope and
;; custom-ui-set-param-by-name-in-scope), and Rust tests that eval the flat
;; spellings (src/ui/state_values/tests.rs). Bare callers cannot see
;; qualified names, so the spellings stay put. `%`-private helpers get none.

;; The current-instrument/current-fx globals below are pinned to eseq.vanilla
;; by their owner (effects/state.lisp, spec §10 hazard i): src/ui/custom_ui.rs
;; GENERATES lisp that `set!`s them by bare name from implicit-module units.
;; They are mutable plain defs, so a bare read here would intern this module's
;; own slot and freeze on the first heal (hazard m) — every read below uses
;; the qualified `eseq.vanilla/` spelling, which reduces to the flat slot the
;; codegen writes (the custom-ui-sections precedent).

(def inst-param (inst name)
  (find-by-key (get inst :synth) :name name))

(def inst-tensor-param (inst name)
  (find-by-key (get inst :tensors) :name name))

(def inst-base-note-param (inst)
  (find-by-key (get inst :synth) :control "base-note"))

(def inst-param-row (inst name key)
  (let ((p (inst-param inst name)))
    (if p
      (pg/fx-param-row p false key)
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def ui-param-control (name)
  (let ((p (inst-param eseq.vanilla/synth-ui-current-inst name)))
    (if p
      (pg/fx-param-row p false (str "custom-ui-" eseq.vanilla/synth-ui-current-name "-" name))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def custom-ui-scope-name ()
  (if (= eseq.vanilla/custom-ui-current-kind "audio-fx")
    (if (get eseq.vanilla/audio-fx-ui-current-fx :bus-fx)
      (str eseq.vanilla/audio-fx-ui-current-name "-bus-" (get eseq.vanilla/audio-fx-ui-current-fx :bus-idx)
           "-slot-" (get eseq.vanilla/audio-fx-ui-current-fx :slot-idx))
      (str eseq.vanilla/audio-fx-ui-current-name "-slot-" (get eseq.vanilla/audio-fx-ui-current-fx :slot-idx)))
    (if (= eseq.vanilla/custom-ui-current-kind "midi-fx")
      (str "midi-" eseq.vanilla/midi-fx-ui-current-name "-slot-" (get eseq.vanilla/midi-fx-ui-current-fx :slot-idx))
      eseq.vanilla/synth-ui-current-name)))

(def custom-ui-current-scope ()
  (dict
    :kind eseq.vanilla/custom-ui-current-kind
    :name (custom-ui-scope-name)
    :audio-fx eseq.vanilla/audio-fx-ui-current-fx
    :midi-fx eseq.vanilla/midi-fx-ui-current-fx
    :inst eseq.vanilla/synth-ui-current-inst))

;; MIDI effect UIs (content/midi-fx/**/ui.lisp) share the lego vocabulary
;; with audio effects: the codegen sets custom-ui-current-kind to "midi-fx"
;; and the param/fx lookups below route to the midi-fx slot dict, whose
;; `:midi-fx true` flag makes param-controls issue set-midi-fx-* commands.

;; A resolved parameter carries its owner across independently rerendered
;; subtrees. Ambient render globals belong to whichever panel rendered last;
;; never consult them again when reading or editing an already resolved param.
;; Keep only the section identity and effect owner: instrument routing is
;; already in p, and retaining the entire instrument here would make every
;; subtree compare every instrument parameter when checking cached inputs.
;; merge creates a descriptor without mutating the host-published parameter.
(def scoped-param (scope p)
  (if p
    (merge p :custom-ui-owner (dict :name (get scope :name) :fx (fx-in-scope scope)))
    false))

(def param-owner (p)
  (get p :custom-ui-owner))

(def custom-ui-param-in-scope (scope name)
  (scoped-param scope
    (if (= (get scope :kind) "audio-fx")
      (fxui/audio-fx-ui-param (get scope :audio-fx) name)
      (if (= (get scope :kind) "midi-fx")
        (fxui/midi-fx-ui-param (get scope :midi-fx) name)
        (inst-param (get scope :inst) name)))))

(def effect-scope? (scope)
  (or (= (get scope :kind) "audio-fx") (= (get scope :kind) "midi-fx")))

(def tensor-param-in-scope (scope name)
  (if (effect-scope? scope)
    false
    (scoped-param scope (inst-tensor-param (get scope :inst) name))))

(def base-note-param-in-scope (scope)
  (if (effect-scope? scope)
    false
    (scoped-param scope (inst-base-note-param (get scope :inst)))))

(def fx-in-scope (scope)
  (if (= (get scope :kind) "audio-fx")
    (get scope :audio-fx)
    (if (= (get scope :kind) "midi-fx")
      (get scope :midi-fx)
      false)))

(def param-fx (p)
  (get (param-owner p) :fx))

(def custom-ui-set-param-in-scope (scope p value)
  (pc/param-set-control-value (fx-in-scope scope) p value))

(def custom-ui-set-param-by-name-in-scope (scope name value)
  (let ((p (custom-ui-param-in-scope scope name)))
    (if p (custom-ui-set-param-in-scope scope p value) false)))

(def custom-ui-set-envelope-in-scope (scope bindings env)
  (let ((pairs (filter (lambda (pair) (nth pair 0))
          (map (lambda (binding)
            (list (custom-ui-param-in-scope scope (nth binding 1)) (get env (nth binding 0)))) bindings)))
        (fx (fx-in-scope scope)))
    (let ((updates (map (lambda (pair)
            (dict :param-idx (get (nth pair 0) :idx) :value (nth pair 1))) pairs)))
      (if (= (len pairs) 0) false
        (if (and fx (not (get fx :rack-fx)) (not (get fx :bus-fx)) (not (get fx :midi-fx)))
          (host-command
            (if (seq-has-selection?) "set-effect-plock-batch" "set-effect-param-batch")
            (dict :slot-idx (get fx :slot-idx)
                  :target-node-id (get fx :target-node-id)
                  :updates updates :commit (not (get env :active))))
          (if (and (not fx) (not (pc/instrument-rack-target? (nth (nth pairs 0) 0))))
            (host-command
              (if (seq-has-selection?) "set-instrument-plock-batch" "set-instrument-param-batch")
              (dict :updates updates :commit (not (get env :active))))
            (map (lambda (pair)
              (custom-ui-set-param-in-scope scope (nth pair 0) (nth pair 1))) pairs)))))))

(def custom-ui-set-adsr-in-scope (scope attack decay sustain release env)
  (custom-ui-set-envelope-in-scope scope
    (if release
      (list (list :attack attack) (list :decay decay) (list :sustain sustain) (list :release release))
      (list (list :attack attack) (list :decay decay) (list :sustain sustain))) env))

(def custom-ui-param-change-callback (p)
  (lambda (v) (custom-ui-set-param p v)))

(def custom-ui-param-change-callback-s (section p)
  (lambda (v)
    (do
      (sec/custom-ui-select-section-in-scope (param-owner p) section)
      (custom-ui-set-param p v))))

(def custom-ui-xy-change-callback-s (section x-p y-p)
  (lambda (x y)
    (do
      (sec/custom-ui-select-section-in-scope (param-owner x-p) section)
      (custom-ui-set-param x-p x)
      (custom-ui-set-param y-p y))))

(def custom-ui-current-param (name)
  (custom-ui-param-in-scope (custom-ui-current-scope) name))

(def custom-ui-current-tensor-param (name)
  (tensor-param-in-scope (custom-ui-current-scope) name))

(def custom-ui-current-base-note-param ()
  (base-note-param-in-scope (custom-ui-current-scope)))

(def custom-ui-set-param (p value)
  (pc/param-set-control-value (param-fx p) p value))

(def custom-ui-param-binding (p)
  (pc/fx-param-value-for (param-fx p) p))

;; Public custom-UI calculations have historically consumed a number here.
;; Keep that contract distinct from the binding passed directly to widgets.
(def custom-ui-param-value (p)
  (reactive-value (custom-ui-param-binding p)))

(def custom-ui-param-control-min (p)
  (pc/param-control-min (param-fx p) p))

(def custom-ui-param-control-max (p)
  (pc/param-control-max (param-fx p) p))

(def custom-ui-param-control-unit (p)
  (pc/param-control-unit (param-fx p) p))

(def custom-ui-param-mod-wrapper (p key body)
  (pc/param-mod-wrapper (param-fx p) p key body))

(def custom-ui-param-control-key-mode (p)
  (pc/param-control-key-mode (param-fx p) p))

(def custom-ui-param-base-value-prop (p)
  (pc/param-base-value-prop (param-fx p) p))

;; Live modulation offset for a custom instrument's param (eseq-6mva). The host
;; samples the most recently triggered voice's modulator and publishes
;; `sum(depth * mod)`; the knob draws its live dot that far from its base.
;; `false` for params with no published field, which draws no dot.
(def custom-ui-param-mod-offset (p)
  (pc/param-mod-offset p))

;; The exponential companion of the offset; see `pc/param-mod-scale`.
(def custom-ui-param-mod-scale (p)
  (pc/param-mod-scale p))

(def custom-ui-param-base-min-prop (p)
  (pc/param-base-min-prop (param-fx p) p))

(def custom-ui-param-base-max-prop (p)
  (pc/param-base-max-prop (param-fx p) p))

(def custom-ui-param-plock-active? (p)
  (pc/param-plock-active? (param-fx p) p))

(def custom-ui-param-plock-default (p)
  (pc/param-plock-default (param-fx p) p))

;; P-lock colour, else the process accent when a step process is mapped to
;; this param (eseq-p1kg), else `:dim`.
(def custom-ui-param-plock-text-color (p)
  (pc/param-process-text-color (param-fx p) p))

;; Process effective value / clamp flag for the knob dot and picker bar
;; (eseq-p1kg); see `pc/param-process-value`.
(def custom-ui-param-process-value (p)
  (pc/param-process-value p))

(def custom-ui-param-process-clamped (p)
  (pc/param-process-clamped p))

(def custom-ui-param-process-mapped? (p)
  (pc/param-process-mapped? p))

;; True while the mods tab paints its dark highlight box behind this param,
;; so light-panel surfaces can swap their black ink for a legible color.
(def custom-ui-param-mod-highlighted? (p)
  (and p (pc/param-mods-open? (param-fx p)) (get p :modulatable)))

(def custom-ui-param-knob-mod-slot-prop (p idx)
  (pc/param-knob-mod-slot-prop (param-fx p) p idx))

(def custom-ui-param-knob-mod-depth-prop (p idx)
  (pc/param-knob-mod-depth-prop (param-fx p) p idx))

(def custom-ui-selected-mod-slot-prop (p)
  (pc/param-selected-mod-slot-prop (param-fx p) p))

(def set-param-by-name (name value)
  (let ((p (custom-ui-current-param name)))
    (if p (custom-ui-set-param p value) false)))

(def custom-ui-tensor-bound-values (p)
  (let ((field (get p :value-field))
        (cells (* (get p :rows) (get p :cols))))
    (map |idx| (bind-seq-nth field idx) (range cells))))

(def custom-ui-tensor-cell-change-callback (p)
  (lambda (row col value)
    (host-command "set-instrument-tensor-cell"
      (dict :tensor-idx (get p :idx)
            :row row
            :col col
            :cell-idx (+ (* row (get p :cols)) col)
            :value value))))

(def custom-ui-tensor-cell-change-callback-s (section p)
  (let ((scope (param-owner p)))
    (lambda (row col value)
      (do
        (sec/custom-ui-select-section-in-scope scope section)
        (host-command "set-instrument-tensor-cell"
          (dict :tensor-idx (get p :idx)
                :row row
                :col col
                :cell-idx (+ (* row (get p :cols)) col)
                :value value))))))

(def base-note ()
  (let ((p (inst-base-note-param eseq.vanilla/synth-ui-current-inst)))
    (if p
      (subtree :key (str "custom-ui-base-note-" eseq.vanilla/synth-ui-current-name)
        (knob-number :label "note"
          :value (pc/fx-param-value p)
          :min (pc/instrument-param-control-min p) :max (pc/instrument-param-control-max p) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width 4.4 :height 2.4
          :value-align :center
          :on-change (lambda (v) (pc/instrument-set-param-control-value p v))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
