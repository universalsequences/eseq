;; Runtime binding helpers exposed to generated custom instrument and effect UIs.
(def inst-param (inst name)
  (nth (filter |p| (= (get p :name) name) (get inst :synth)) 0))

(def inst-tensor-param (inst name)
  (nth (filter |p| (= (get p :name) name) (get inst :tensors)) 0))

(def inst-base-note-param (inst)
  (nth (filter |p| (= (get p :control) "base-note") (get inst :synth)) 0))

(def inst-param-row (inst name key)
  (let ((p (inst-param inst name)))
    (if p
      (fx-param-row p false key)
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def ui-param-control (name)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (fx-param-row p false (str "custom-ui-" synth-ui-current-name "-" name))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def custom-ui-scope-name ()
  (if (= custom-ui-current-kind "audio-fx")
    (if (get audio-fx-ui-current-fx :bus-fx)
      (str audio-fx-ui-current-name "-bus-" (get audio-fx-ui-current-fx :bus-idx)
           "-slot-" (get audio-fx-ui-current-fx :slot-idx))
      (str audio-fx-ui-current-name "-slot-" (get audio-fx-ui-current-fx :slot-idx)))
    synth-ui-current-name))

(def custom-ui-current-scope ()
  (dict
    :kind custom-ui-current-kind
    :name (custom-ui-scope-name)
    :audio-fx audio-fx-ui-current-fx
    :inst synth-ui-current-inst))

(def custom-ui-param-in-scope (scope name)
  (if (= (get scope :kind) "audio-fx")
    (audio-fx-ui-param (get scope :audio-fx) name)
    (inst-param (get scope :inst) name)))

(def custom-ui-tensor-param-in-scope (scope name)
  (if (= (get scope :kind) "audio-fx")
    false
    (inst-tensor-param (get scope :inst) name)))

(def custom-ui-base-note-param-in-scope (scope)
  (if (= (get scope :kind) "audio-fx")
    false
    (inst-base-note-param (get scope :inst))))

(def custom-ui-fx-in-scope (scope)
  (if (= (get scope :kind) "audio-fx")
    (get scope :audio-fx)
    false))

(def custom-ui-current-fx ()
  (custom-ui-fx-in-scope (custom-ui-current-scope)))

(def custom-ui-set-param-in-scope (scope p value)
  (param-set-control-value (custom-ui-fx-in-scope scope) p value))

(def custom-ui-set-param-by-name-in-scope (scope name value)
  (let ((p (custom-ui-param-in-scope scope name)))
    (if p (custom-ui-set-param-in-scope scope p value) false)))

(def custom-ui-param-change-callback (p)
  (let ((scope (custom-ui-current-scope)))
    (lambda (v)
      (custom-ui-set-param-in-scope scope p v))))

(def custom-ui-param-change-callback-s (section p)
  (let ((scope (custom-ui-current-scope)))
    (lambda (v)
      (do
        (custom-ui-select-section-in-scope scope section)
        (custom-ui-set-param-in-scope scope p v)))))

(def custom-ui-current-param (name)
  (if (= custom-ui-current-kind "audio-fx")
    (audio-fx-ui-param audio-fx-ui-current-fx name)
    (inst-param synth-ui-current-inst name)))

(def custom-ui-current-tensor-param (name)
  (if (= custom-ui-current-kind "audio-fx")
    false
    (inst-tensor-param synth-ui-current-inst name)))

(def custom-ui-current-base-note-param ()
  (if (= custom-ui-current-kind "audio-fx")
    false
    (inst-base-note-param synth-ui-current-inst)))

(def custom-ui-set-param (p value)
  (custom-ui-set-param-in-scope (custom-ui-current-scope) p value))

(def custom-ui-param-binding (p)
  (fx-param-value-for (custom-ui-current-fx) p))

;; Public custom-UI calculations have historically consumed a number here.
;; Keep that contract distinct from the binding passed directly to widgets.
(def custom-ui-param-value (p)
  (reactive-value (custom-ui-param-binding p)))

(def custom-ui-param-control-min (p)
  (param-control-min (custom-ui-current-fx) p))

(def custom-ui-param-control-max (p)
  (param-control-max (custom-ui-current-fx) p))

(def custom-ui-param-mod-wrapper (p key body)
  (param-mod-wrapper (custom-ui-current-fx) p key body))

(def custom-ui-param-control-key-mode (p)
  (param-control-key-mode (custom-ui-current-fx) p))

(def custom-ui-param-base-value-prop (p)
  (param-base-value-prop (custom-ui-current-fx) p))

(def custom-ui-param-base-min-prop (p)
  (param-base-min-prop (custom-ui-current-fx) p))

(def custom-ui-param-base-max-prop (p)
  (param-base-max-prop (custom-ui-current-fx) p))

(def custom-ui-param-plock-active? (p)
  (param-plock-active? (custom-ui-current-fx) p))

(def custom-ui-param-plock-default (p)
  (param-plock-default (custom-ui-current-fx) p))

(def custom-ui-param-plock-text-color (p)
  (param-plock-text-color (custom-ui-current-fx) p))

(def custom-ui-param-knob-mod-slot-prop (p idx)
  (param-knob-mod-slot-prop (custom-ui-current-fx) p idx))

(def custom-ui-param-knob-mod-depth-prop (p idx)
  (param-knob-mod-depth-prop (custom-ui-current-fx) p idx))

(def custom-ui-selected-mod-slot-prop (p)
  (param-selected-mod-slot-prop (custom-ui-current-fx) p))

(def custom-ui-set-param-by-name (name value)
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
        (custom-ui-select-section-in-scope scope section)
        (host-command "set-instrument-tensor-cell"
          (dict :tensor-idx (get p :idx)
                :row row
                :col col
                :cell-idx (+ (* row (get p :cols)) col)
                :value value))))))

(def base-note ()
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key (str "custom-ui-base-note-" synth-ui-current-name)
        (knob-number :label "note"
          :value (fx-param-value p)
          :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width 4.4 :height 2.4
          :value-align :center
          :on-change (lambda (v) (instrument-set-param-control-value p v))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))
