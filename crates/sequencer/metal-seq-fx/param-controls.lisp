;; Parameter value routing, modulation targeting, and wrappers.
(def instrument-rack-target? (p)
  (not (= (get p :rack-track) nil)))

(def instrument-target-param-dict (source-p idx)
  (if (instrument-rack-target? source-p)
    (dict :idx idx :control "param"
          :rack-track (get source-p :rack-track)
          :rack-slot (get source-p :rack-slot))
    (dict :idx idx :control "param")))

(def instrument-keys-active? ()
  (= instrument-panel-tab 1))

(def instrument-key-lock-has-selection? ()
  (> (len instrument-key-lock-selected-notes) 0))

(def instrument-key-lock-authoring-active? ()
  (and (instrument-keys-active?) (instrument-key-lock-has-selection?)))

(def instrument-selected-key-note ()
  (nth instrument-key-lock-selected-notes 0))

(def instrument-param-key-lock-row (p note)
  (nth (filter |row| (= (get row :note) note) (get p :key-locks))
    0))

(def instrument-param-key-lock-active? (p)
  (let ((note (instrument-selected-key-note)))
    (if note
      (if (instrument-param-key-lock-row p note) true false)
      false)))

(def instrument-param-base-value (p)
  (if (get p :value-field)
    (bind-seq (get p :value-field))
    (get p :value)))

(def instrument-param-key-lock-value (p)
  (let ((note (instrument-selected-key-note)))
    (if note
      (let ((row (instrument-param-key-lock-row p note)))
        (if row (get row :value) (instrument-param-base-value p)))
      (instrument-param-base-value p))))

(def fx-set-instrument-value (p v)
  (do
    (fx-clear-selected-effect)
    (let ((rack-track (get p :rack-track))
          (rack-slot (get p :rack-slot)))
      (if (instrument-rack-target? p)
        (if (= (get p :control) "base-note")
          (host-command (if (seq-has-selection?) "set-rack-slot-param-plock" "set-rack-slot-base-note")
            (dict :track rack-track :slot rack-slot :param "base-note" :value v))
          (host-command
            (if (seq-has-selection?) "set-rack-slot-instrument-plock" "set-rack-slot-instrument-param")
            (dict :track rack-track :slot rack-slot :param-idx (get p :idx) :value v)))
        (if (= (get p :control) "base-note")
          (host-command "set-instrument-base-note" (dict :value v))
          (host-command
            (if (instrument-key-lock-authoring-active?)
              "set-instrument-key-lock-multi"
              (if (seq-has-selection?) "set-instrument-plock" "set-instrument-param"))
            (dict :param-idx (get p :idx) :value v :notes instrument-key-lock-selected-notes)))))))

(def fx-set-instrument-option (p label)
  (do
    (fx-clear-selected-effect)
    (let ((rack-track (get p :rack-track))
          (rack-slot (get p :rack-slot)))
      (if (instrument-rack-target? p)
        (host-command
          (if (seq-has-selection?) "set-rack-slot-instrument-plock-option" "set-rack-slot-instrument-param-option")
          (dict :track rack-track :slot rack-slot :param-idx (get p :idx) :label label))
        (host-command
          (if (instrument-key-lock-authoring-active?)
            "set-instrument-key-lock-option-multi"
            (if (seq-has-selection?) "set-instrument-plock-option" "set-instrument-param-option"))
          (dict :param-idx (get p :idx) :label label :notes instrument-key-lock-selected-notes))))))

(def custom-ui-option-index (options label)
  (nth (filter |idx| (= (nth options idx) label) (range (len options))) 0))

(def fx-set-effect-value (fx p v)
  (do
    (fx-clear-selected-effect)
    (if (get fx :bus-fx)
      (host-command (if (seq-has-selection?) "set-bus-effect-plock" "set-bus-effect-param")
        (dict :bus (get fx :bus-idx) :slot-idx (get fx :slot-idx)
              :param-idx (get p :idx) :value v))
    (if (get fx :midi-fx)
      (host-command
        (if (seq-has-selection?) "set-midi-fx-plock" "set-midi-fx-param")
        (dict :slot-idx (get fx :slot-idx) :param-idx (get p :idx) :value v))
      (if (seq-has-selection?)
        (seq-set-effect-plock (get fx :slot-idx) (get p :idx) v)
        (host-command "set-effect-param"
          (dict :slot-idx (get fx :slot-idx) :param-idx (get p :idx) :value v)))))))

(def fx-toggle-instrument-value (p)
  (do
    (fx-clear-selected-effect)
    (let ((rack-track (get p :rack-track))
          (rack-slot (get p :rack-slot)))
      (if (instrument-rack-target? p)
        (host-command
          (if (seq-has-selection?) "toggle-rack-slot-instrument-plock" "toggle-rack-slot-instrument-param")
          (dict :track rack-track :slot rack-slot :param-idx (get p :idx)))
        (if (instrument-key-lock-authoring-active?)
          (host-command "set-instrument-key-lock-multi"
            (dict :param-idx (get p :idx)
                  :notes instrument-key-lock-selected-notes
                  :value (if (fx-param-on? p) 0 1)))
          (host-command "toggle-instrument-param"
            (dict :param-idx (get p :idx))))))))

(def fx-toggle-effect-value (fx p)
  (do
    (fx-clear-selected-effect)
    (host-command "toggle-effect-param"
      (dict :bus (get fx :bus-idx)
            :bus-fx (get fx :bus-fx)
            :midi-fx (get fx :midi-fx)
            :slot-idx (get fx :slot-idx)
            :param-idx (get p :idx)))))

(def fx-param-has-idx? (p)
  (not (= (get p :idx) nil)))

(def fx-param-value (p)
  (if (and (instrument-keys-active?) (fx-param-has-idx? p))
    (instrument-param-key-lock-value p)
    (if (and instrument-mods-open (get p :modulatable))
    (let ((target (instrument-param-control-mod-target p)))
      (if target (instrument-mod-target-depth target) 0))
    (instrument-param-base-value p))))

(def effect-mods-active? (fx)
  (and fx
       effect-mods-open
       (= effect-mods-chain (fx-effect-chain-kind fx))
       (= effect-mods-slot (get fx :slot-idx))
       (= effect-mods-bus (if (get fx :bus-fx) (get fx :bus-idx) -1))))

(def fx-has-modulators? (fx)
  (and fx (> (len (get fx :sources)) 0)))

(def param-mods-open? (fx)
  (if fx (effect-mods-active? fx) instrument-mods-open))

(def param-mod-selected-slot (fx)
  (if fx
    (if (> effect-selected-mod-slot 0) effect-selected-mod-slot 1)
    (instrument-mod-selected-slot)))

(def param-selected-mod-target (fx p)
  (nth
    (filter |target| (= (instrument-mod-target-source-slot target) (param-mod-selected-slot fx))
      (instrument-param-mod-targets p))
    0))

(def param-empty-mod-target (p)
  (nth
    (filter |target| (and (get target :source-idx)
                          (= (instrument-mod-target-source-slot target) 0))
      (instrument-param-mod-targets p))
    0))

(def param-control-mod-target (fx p)
  (let ((selected-target (param-selected-mod-target fx p)))
    (if selected-target
      selected-target
      (let ((empty-target (param-empty-mod-target p)))
        (if empty-target
          empty-target
          (nth (instrument-param-mod-targets p) 0))))))

(def fx-param-value-for (fx p)
  (if (and (not fx) (instrument-keys-active?) (fx-param-has-idx? p))
    (instrument-param-key-lock-value p)
    (if (and (param-mods-open? fx) (get p :modulatable))
    (let ((target (param-control-mod-target fx p)))
      (if target (instrument-mod-target-depth target) 0))
    (if (get p :value-field)
      (bind-seq (get p :value-field))
      (get p :value)))))

(def fx-param-text-value-for (fx p)
  (if (and (not fx) (instrument-keys-active?) (get p :options))
    (nth (get p :options) (fx-param-value-for fx p))
    (get p :text-value)))

(def param-plock-row-target (fx)
  (if fx
    (if (get fx :midi-fx) "midi-fx" "effect")
    "instrument"))

(def param-plock-row (fx p)
  (if (get p :idx)
    (let ((target (param-plock-row-target fx))
          (slot (if fx (get fx :slot-idx) -1))
          (idx (get p :idx)))
      (nth (filter |row|
        (and (= (get row :target) target)
             (= (get row :param-idx) idx)
             (if fx (= (get row :slot-idx) slot) true))
        SEQ.track-plocks) 0))
    false))

(def param-current-variant-chip ()
  (nth (filter |chip| (and (get chip :current)
                           (= (get chip :kind) "variant"))
        SEQ.track-plock-variants) 0))

(def param-plock-active? (fx p)
  (if (and (not fx) (instrument-keys-active?))
    (instrument-param-key-lock-active? p)
    (and (not (param-mods-open? fx))
         (param-plock-row fx p))))

(def param-plock-default (fx p)
  (let ((row (param-plock-row fx p)))
    (if row (get row :default) (fx-param-value-for fx p))))

(def param-plock-color-r ()
  (let ((chip (param-current-variant-chip)))
    (if chip (get chip :color-r) 0.27058825)))

(def param-plock-color-g ()
  (let ((chip (param-current-variant-chip)))
    (if chip (get chip :color-g) 0.78431374)))

(def param-plock-color-b ()
  (let ((chip (param-current-variant-chip)))
    (if chip (get chip :color-b) 0.8627451)))

(def param-plock-text-color (fx p)
  (if (param-plock-active? fx p)
    (rgba (param-plock-color-r) (param-plock-color-g) (param-plock-color-b) 1.0)
    :dim))

(def param-control-min (fx p)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (let ((target (param-control-mod-target fx p)))
      (if target (get target :depth-min) -1))
    (get p :min)))

(def param-control-max (fx p)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (let ((target (param-control-mod-target fx p)))
      (if target (get target :depth-max) 1))
    (get p :max)))

(def param-set-option (fx p label)
  (if fx
    (do
      (fx-clear-selected-effect)
      (host-command
        (if (get fx :bus-fx)
          (if (seq-has-selection?) "set-bus-effect-plock-option" "set-bus-effect-param-option")
          (if (get fx :midi-fx)
            (if (seq-has-selection?) "set-midi-fx-plock-option" "set-midi-fx-param-option")
            (if (seq-has-selection?) "set-effect-plock-option" "set-effect-param-option")))
        (dict :bus (get fx :bus-idx) :slot-idx (get fx :slot-idx)
              :param-idx (get p :idx) :label label)))
    (fx-set-instrument-option p label)))

(def param-set-control-value (fx p v)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (let ((target (param-control-mod-target fx p)))
      (if target
        (let ((source-slot (instrument-mod-target-source-slot target))
              (selected-slot (param-mod-selected-slot fx)))
          (if (= source-slot selected-slot)
            (if fx
              (fx-set-effect-value fx (dict :idx (get target :depth-idx) :control "param") v)
              (fx-set-instrument-value (instrument-target-param-dict p (get target :depth-idx)) v))
            (if (= source-slot 0)
              (do
                (if fx
                  (fx-set-effect-value fx (dict :idx (get target :source-idx) :control "param") selected-slot)
                  (fx-set-instrument-value (instrument-target-param-dict p (get target :source-idx)) selected-slot))
                (if fx
                  (fx-set-effect-value fx (dict :idx (get target :depth-idx) :control "param") v)
                  (fx-set-instrument-value (instrument-target-param-dict p (get target :depth-idx)) v))))))))
    (if fx (fx-set-effect-value fx p v) (fx-set-instrument-value p v))))

(def param-toggle-modulation (fx p)
  (if (get p :modulatable)
    (let ((target (param-selected-mod-target fx p))
          (selected-slot (param-mod-selected-slot fx)))
      (if target
        (if (get target :source-idx)
          (if fx
            (fx-set-effect-value fx (dict :idx (get target :source-idx) :control "param") 0)
            (fx-set-instrument-value (instrument-target-param-dict p (get target :source-idx)) 0))
          (if fx
            (fx-set-effect-value fx (dict :idx (get target :depth-idx) :control "param") 0)
            (fx-set-instrument-value (instrument-target-param-dict p (get target :depth-idx)) 0)))
        (let ((target (param-empty-mod-target p)))
          (if target
            (do
              (if fx
                (fx-set-effect-value fx (dict :idx (get target :source-idx) :control "param") selected-slot)
                (fx-set-instrument-value (instrument-target-param-dict p (get target :source-idx)) selected-slot))
              (if fx
                (fx-set-effect-value fx (dict :idx (get target :depth-idx) :control "param") 0)
                (fx-set-instrument-value (instrument-target-param-dict p (get target :depth-idx)) 0)))))))))

(def param-mod-bg (fx p)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (rgba 0.18 0.48 0.95 0.24)
    :transparent))

(def param-mod-wrapper (fx p key body)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (subtree :key key
      (box :background-color (param-mod-bg fx p)
           :corner-radius 8
           :border-width 1
           :padding 0.08
           :on-double-click (lambda (info) (param-toggle-modulation fx p))
        body))
    body))

(def fx-param-numeric-value (p)
  (reactive-value (fx-param-value p)))

(def fx-param-on? (p)
  (> (fx-param-numeric-value p) 0.5))

(def instrument-mod-selected-slot ()
  (if (> instrument-selected-mod-slot 0) instrument-selected-mod-slot 1))

(def instrument-param-mod-targets (p)
  (if (get p :mod-targets) (get p :mod-targets) '()))

(def instrument-mod-target-source-slot (target)
  (let ((slot (if (get target :source-value-field)
                (reactive-value (bind-seq (get target :source-value-field)))
                (get target :source-slot))))
    (if slot slot (get target :source-slot))))

(def instrument-mod-target-depth (target)
  (if (get target :depth-value-field)
    (bind-seq (get target :depth-value-field))
    (get target :depth)))

(def param-knob-mod-target (fx p idx)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (nth (instrument-param-mod-targets p) idx)
    false))

(def param-knob-mod-slot-prop (fx p idx)
  (let ((target (param-knob-mod-target fx p idx)))
    (if target (instrument-mod-target-source-slot target) false)))

(def param-knob-mod-depth-prop (fx p idx)
  (let ((target (param-knob-mod-target fx p idx)))
    (if target (instrument-mod-target-depth target) false)))

(def param-base-value-prop (fx p)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (instrument-param-base-value p)
    false))

(def param-base-min-prop (fx p)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (get p :min)
    false))

(def param-base-max-prop (fx p)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (get p :max)
    false))

(def param-selected-mod-slot-prop (fx p)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (param-mod-selected-slot fx)
    false))

(def param-control-key-mode (fx p)
  (if (and (param-mods-open? fx) (get p :modulatable))
    "-mod-depth"
    "-base"))

(def instrument-param-base-value (p)
  (if (get p :value-field)
    (bind-seq (get p :value-field))
    (get p :value)))

(def instrument-param-active-mod-targets (p)
  (if (and instrument-mods-open (get p :modulatable))
    (filter |target| (> (instrument-mod-target-source-slot target) 0)
      (instrument-param-mod-targets p))
    '()))

(def instrument-param-knob-mod-target (p idx)
  (param-knob-mod-target false p idx))

(def instrument-param-knob-mod-slot-prop (p idx)
  (param-knob-mod-slot-prop false p idx))

(def instrument-param-knob-mod-depth-prop (p idx)
  (param-knob-mod-depth-prop false p idx))

(def instrument-param-base-value-prop (p)
  (param-base-value-prop false p))

(def instrument-param-base-min-prop (p)
  (param-base-min-prop false p))

(def instrument-param-base-max-prop (p)
  (param-base-max-prop false p))

(def instrument-selected-mod-slot-prop (p)
  (param-selected-mod-slot-prop false p))

(def instrument-param-control-key-mode (p)
  (param-control-key-mode false p))

(def instrument-param-selected-mod-target (p)
  (nth
    (filter |target| (= (instrument-mod-target-source-slot target) (instrument-mod-selected-slot))
      (instrument-param-mod-targets p))
    0))

(def instrument-param-empty-mod-target (p)
  (nth
    (filter |target| (and (get target :source-idx)
                          (= (instrument-mod-target-source-slot target) 0))
      (instrument-param-mod-targets p))
    0))

(def instrument-param-control-mod-target (p)
  (let ((selected-target (instrument-param-selected-mod-target p)))
    (if selected-target
      selected-target
      (let ((empty-target (instrument-param-empty-mod-target p)))
        (if empty-target
          empty-target
          (nth (instrument-param-mod-targets p) 0))))))

(def instrument-param-connected-to-selected-mod? (p)
  (if (instrument-param-selected-mod-target p) true false))

(def instrument-param-connected-to-other-mod? (p)
  (> (len
      (filter |target|
        (and (> (instrument-mod-target-source-slot target) 0)
             (not (= (instrument-mod-target-source-slot target) (instrument-mod-selected-slot))))
        (instrument-param-mod-targets p)))
     0))

(def instrument-param-control-min (p)
  (if (and instrument-mods-open (get p :modulatable))
    (let ((target (instrument-param-control-mod-target p)))
      (if target (get target :depth-min) -1))
    (get p :min)))

(def instrument-param-control-max (p)
  (if (and instrument-mods-open (get p :modulatable))
    (let ((target (instrument-param-control-mod-target p)))
      (if target (get target :depth-max) 1))
    (get p :max)))

(def instrument-set-param-control-value (p v)
  (if (and instrument-mods-open (get p :modulatable))
    (let ((target (instrument-param-control-mod-target p)))
      (if target
        (let ((source-slot (instrument-mod-target-source-slot target)))
          (if (= source-slot (instrument-mod-selected-slot))
            (fx-set-instrument-value
              (instrument-target-param-dict p (get target :depth-idx))
              v)
            (if (= source-slot 0)
              (do
                (fx-set-instrument-value
                  (instrument-target-param-dict p (get target :source-idx))
                  (instrument-mod-selected-slot))
                (fx-set-instrument-value
                  (instrument-target-param-dict p (get target :depth-idx))
                  v)))))))
    (fx-set-instrument-value p v)))

(def instrument-toggle-param-modulation (p)
  (if (get p :modulatable)
    (let ((target (instrument-param-selected-mod-target p)))
      (if target
        (if (get target :source-idx)
          (fx-set-instrument-value
            (instrument-target-param-dict p (get target :source-idx))
            0)
          (fx-set-instrument-value
            (instrument-target-param-dict p (get target :depth-idx))
            0))
        (let ((target (instrument-param-empty-mod-target p)))
          (if target
            (do
              (fx-set-instrument-value
                (instrument-target-param-dict p (get target :source-idx))
                (instrument-mod-selected-slot))
              (fx-set-instrument-value
                (instrument-target-param-dict p (get target :depth-idx))
                0))))))))

(def instrument-param-mod-bg (p)
  (if (and instrument-mods-open (get p :modulatable))
    (rgba 0.18 0.48 0.95 0.24)
    :transparent))

(def instrument-param-mod-wrapper (p key body)
  (if (and instrument-mods-open (get p :modulatable))
    (subtree :key key
      (box :background-color (instrument-param-mod-bg p)
           :corner-radius 8
           :border-width 1
           :padding 0.08
           :on-double-click (lambda (info) (instrument-toggle-param-modulation p))
        body))
    body))
