;; Runtime binding helpers exposed to generated custom instrument and effect UIs.
(def inst-param (inst name)
  (nth (filter |p| (= (get p :name) name) (get inst :synth)) 0))

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

(def custom-ui-base-note-param-in-scope (scope)
  (if (= (get scope :kind) "audio-fx")
    false
    (inst-base-note-param (get scope :inst))))

(def custom-ui-set-param-in-scope (scope p value)
  (if (= (get scope :kind) "audio-fx")
    (fx-set-effect-value (get scope :audio-fx) p value)
    (instrument-set-param-control-value p value)))

(def custom-ui-set-param-by-name-in-scope (scope name value)
  (let ((p (custom-ui-param-in-scope scope name)))
    (if p (custom-ui-set-param-in-scope scope p value) false)))

(def custom-ui-param-change-callback (p)
  (let ((scope (custom-ui-current-scope)))
    (lambda (v)
      (if (= (get scope :kind) "audio-fx")
        (custom-ui-set-param-in-scope scope p v)
        (instrument-set-param-control-value p v)))))

(def custom-ui-param-change-callback-s (section p)
  (let ((scope (custom-ui-current-scope)))
    (lambda (v)
      (do
        (custom-ui-select-section-in-scope scope section)
        (if (= (get scope :kind) "audio-fx")
          (custom-ui-set-param-in-scope scope p v)
          (instrument-set-param-control-value p v))))))

(def custom-ui-current-param (name)
  (if (= custom-ui-current-kind "audio-fx")
    (audio-fx-ui-param audio-fx-ui-current-fx name)
    (inst-param synth-ui-current-inst name)))

(def custom-ui-current-base-note-param ()
  (if (= custom-ui-current-kind "audio-fx")
    false
    (inst-base-note-param synth-ui-current-inst)))

(def custom-ui-set-param (p value)
  (if (= custom-ui-current-kind "audio-fx")
    (custom-ui-set-param-in-scope (custom-ui-current-scope) p value)
    (instrument-set-param-control-value p value)))

(def custom-ui-set-param-by-name (name value)
  (let ((p (custom-ui-current-param name)))
    (if p (custom-ui-set-param p value) false)))

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
