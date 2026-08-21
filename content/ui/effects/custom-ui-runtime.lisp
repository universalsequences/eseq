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
        custom-ui-param-change-callback
        custom-ui-param-change-callback-s
        custom-ui-current-param
        custom-ui-current-tensor-param
        custom-ui-current-base-note-param
        custom-ui-set-param
        custom-ui-param-binding
        custom-ui-param-value
        custom-ui-param-control-min
        custom-ui-param-control-max
        custom-ui-param-mod-wrapper
        custom-ui-param-control-key-mode
        custom-ui-param-base-value-prop
        custom-ui-param-base-min-prop
        custom-ui-param-base-max-prop
        custom-ui-param-plock-active?
        custom-ui-param-plock-default
        custom-ui-param-plock-text-color
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
  (nth (filter |p| (= (get p :name) name) (get inst :synth)) 0))

(def inst-tensor-param (inst name)
  (nth (filter |p| (= (get p :name) name) (get inst :tensors)) 0))

(def inst-base-note-param (inst)
  (nth (filter |p| (= (get p :control) "base-note") (get inst :synth)) 0))

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
    eseq.vanilla/synth-ui-current-name))

(def custom-ui-current-scope ()
  (dict
    :kind eseq.vanilla/custom-ui-current-kind
    :name (custom-ui-scope-name)
    :audio-fx eseq.vanilla/audio-fx-ui-current-fx
    :inst eseq.vanilla/synth-ui-current-inst))

(def custom-ui-param-in-scope (scope name)
  (if (= (get scope :kind) "audio-fx")
    (fxui/audio-fx-ui-param (get scope :audio-fx) name)
    (inst-param (get scope :inst) name)))

(def tensor-param-in-scope (scope name)
  (if (= (get scope :kind) "audio-fx")
    false
    (inst-tensor-param (get scope :inst) name)))

(def base-note-param-in-scope (scope)
  (if (= (get scope :kind) "audio-fx")
    false
    (inst-base-note-param (get scope :inst))))

(def fx-in-scope (scope)
  (if (= (get scope :kind) "audio-fx")
    (get scope :audio-fx)
    false))

(def current-fx ()
  (fx-in-scope (custom-ui-current-scope)))

(def custom-ui-set-param-in-scope (scope p value)
  (pc/param-set-control-value (fx-in-scope scope) p value))

(def custom-ui-set-param-by-name-in-scope (scope name value)
  (let ((p (custom-ui-param-in-scope scope name)))
    (if p (custom-ui-set-param-in-scope scope p value) false)))

(def custom-ui-set-adsr-in-scope (scope attack decay sustain release env)
  (let ((attack-p (custom-ui-param-in-scope scope attack))
        (decay-p (custom-ui-param-in-scope scope decay))
        (sustain-p (custom-ui-param-in-scope scope sustain))
        (release-p (if release (custom-ui-param-in-scope scope release) false))
        (fx (fx-in-scope scope)))
    (let ((updates (if release-p
          (list
            (dict :param-idx (get attack-p :idx) :value (get env :attack))
            (dict :param-idx (get decay-p :idx) :value (get env :decay))
            (dict :param-idx (get sustain-p :idx) :value (get env :sustain))
            (dict :param-idx (get release-p :idx) :value (get env :release)))
          (list
            (dict :param-idx (get attack-p :idx) :value (get env :attack))
            (dict :param-idx (get decay-p :idx) :value (get env :decay))
            (dict :param-idx (get sustain-p :idx) :value (get env :sustain))))))
      (if (and fx (not (get fx :rack-fx)) (not (get fx :bus-fx)) (not (get fx :midi-fx)))
        (host-command
          (if (seq-has-selection?) "set-effect-plock-batch" "set-effect-param-batch")
          (dict :slot-idx (get fx :slot-idx) :updates updates
                :commit (not (get env :active))))
        (if (and (not fx) (not (pc/instrument-rack-target? attack-p)))
          (host-command
            (if (seq-has-selection?) "set-instrument-plock-batch" "set-instrument-param-batch")
            (dict :updates updates :commit (not (get env :active))))
          (do
            (custom-ui-set-param-in-scope scope attack-p (get env :attack))
            (custom-ui-set-param-in-scope scope decay-p (get env :decay))
            (custom-ui-set-param-in-scope scope sustain-p (get env :sustain))
            (if release-p
              (custom-ui-set-param-in-scope scope release-p (get env :release))
              false)))))))

(def custom-ui-param-change-callback (p)
  (let ((scope (custom-ui-current-scope)))
    (lambda (v)
      (custom-ui-set-param-in-scope scope p v))))

(def custom-ui-param-change-callback-s (section p)
  (let ((scope (custom-ui-current-scope)))
    (lambda (v)
      (do
        (sec/custom-ui-select-section-in-scope scope section)
        (custom-ui-set-param-in-scope scope p v)))))

(def custom-ui-current-param (name)
  (if (= eseq.vanilla/custom-ui-current-kind "audio-fx")
    (fxui/audio-fx-ui-param eseq.vanilla/audio-fx-ui-current-fx name)
    (inst-param eseq.vanilla/synth-ui-current-inst name)))

(def custom-ui-current-tensor-param (name)
  (if (= eseq.vanilla/custom-ui-current-kind "audio-fx")
    false
    (inst-tensor-param eseq.vanilla/synth-ui-current-inst name)))

(def custom-ui-current-base-note-param ()
  (if (= eseq.vanilla/custom-ui-current-kind "audio-fx")
    false
    (inst-base-note-param eseq.vanilla/synth-ui-current-inst)))

(def custom-ui-set-param (p value)
  (custom-ui-set-param-in-scope (custom-ui-current-scope) p value))

(def custom-ui-param-binding (p)
  (pc/fx-param-value-for (current-fx) p))

;; Public custom-UI calculations have historically consumed a number here.
;; Keep that contract distinct from the binding passed directly to widgets.
(def custom-ui-param-value (p)
  (reactive-value (custom-ui-param-binding p)))

(def custom-ui-param-control-min (p)
  (pc/param-control-min (current-fx) p))

(def custom-ui-param-control-max (p)
  (pc/param-control-max (current-fx) p))

(def custom-ui-param-mod-wrapper (p key body)
  (pc/param-mod-wrapper (current-fx) p key body))

(def custom-ui-param-control-key-mode (p)
  (pc/param-control-key-mode (current-fx) p))

(def custom-ui-param-base-value-prop (p)
  (pc/param-base-value-prop (current-fx) p))

(def custom-ui-param-base-min-prop (p)
  (pc/param-base-min-prop (current-fx) p))

(def custom-ui-param-base-max-prop (p)
  (pc/param-base-max-prop (current-fx) p))

(def custom-ui-param-plock-active? (p)
  (pc/param-plock-active? (current-fx) p))

(def custom-ui-param-plock-default (p)
  (pc/param-plock-default (current-fx) p))

(def custom-ui-param-plock-text-color (p)
  (pc/param-plock-text-color (current-fx) p))

(def custom-ui-param-knob-mod-slot-prop (p idx)
  (pc/param-knob-mod-slot-prop (current-fx) p idx))

(def custom-ui-param-knob-mod-depth-prop (p idx)
  (pc/param-knob-mod-depth-prop (current-fx) p idx))

(def custom-ui-selected-mod-slot-prop (p)
  (pc/param-selected-mod-slot-prop (current-fx) p))

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
  (let ((scope (custom-ui-current-scope)))
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
