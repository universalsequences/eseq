;; Parameter value routing, modulation targeting, and wrappers.
(def fx-set-instrument-value (p v)
  (do
    (fx-clear-selected-effect)
    (if (= (get p :control) "base-note")
      (host-command "set-instrument-base-note" (dict :value v))
      (host-command
        (if (seq-has-selection?) "set-instrument-plock" "set-instrument-param")
        (dict :param-idx (get p :idx) :value v)))))

(def fx-set-instrument-option (p label)
  (do
    (fx-clear-selected-effect)
    (host-command
      (if (seq-has-selection?) "set-instrument-plock-option" "set-instrument-param-option")
      (dict :param-idx (get p :idx) :label label))))

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
    (host-command "toggle-instrument-param"
      (dict :param-idx (get p :idx)))))

(def fx-toggle-effect-value (fx p)
  (do
    (fx-clear-selected-effect)
    (host-command "toggle-effect-param"
      (dict :bus (get fx :bus-idx)
            :bus-fx (get fx :bus-fx)
            :midi-fx (get fx :midi-fx)
            :slot-idx (get fx :slot-idx)
            :param-idx (get p :idx)))))

(def fx-param-value (p)
  (if (and instrument-mods-open (get p :modulatable))
    (let ((target (instrument-param-control-mod-target p)))
      (if target (instrument-mod-target-depth target) 0))
    (if (get p :value-field)
      (bind-seq (get p :value-field))
      (get p :value))))

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
  (if (and (param-mods-open? fx) (get p :modulatable))
    (let ((target (param-control-mod-target fx p)))
      (if target (instrument-mod-target-depth target) 0))
    (if (get p :value-field)
      (bind-seq (get p :value-field))
      (get p :value))))

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
              (fx-set-instrument-value (dict :idx (get target :depth-idx) :control "param") v))
            (if (= source-slot 0)
              (do
                (if fx
                  (fx-set-effect-value fx (dict :idx (get target :source-idx) :control "param") selected-slot)
                  (fx-set-instrument-value (dict :idx (get target :source-idx) :control "param") selected-slot))
                (if fx
                  (fx-set-effect-value fx (dict :idx (get target :depth-idx) :control "param") v)
                  (fx-set-instrument-value (dict :idx (get target :depth-idx) :control "param") v))))))))
    (if fx (fx-set-effect-value fx p v) (fx-set-instrument-value p v))))

(def param-toggle-modulation (fx p)
  (if (get p :modulatable)
    (let ((target (param-selected-mod-target fx p))
          (selected-slot (param-mod-selected-slot fx)))
      (if target
        (if (get target :source-idx)
          (if fx
            (fx-set-effect-value fx (dict :idx (get target :source-idx) :control "param") 0)
            (fx-set-instrument-value (dict :idx (get target :source-idx) :control "param") 0))
          (if fx
            (fx-set-effect-value fx (dict :idx (get target :depth-idx) :control "param") 0)
            (fx-set-instrument-value (dict :idx (get target :depth-idx) :control "param") 0)))
        (let ((target (param-empty-mod-target p)))
          (if target
            (do
              (if fx
                (fx-set-effect-value fx (dict :idx (get target :source-idx) :control "param") selected-slot)
                (fx-set-instrument-value (dict :idx (get target :source-idx) :control "param") selected-slot))
              (if fx
                (fx-set-effect-value fx (dict :idx (get target :depth-idx) :control "param") 0)
                (fx-set-instrument-value (dict :idx (get target :depth-idx) :control "param") 0)))))))))

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
  (if (and instrument-mods-open (get p :modulatable))
    (nth (instrument-param-mod-targets p) idx)
    false))

(def instrument-param-knob-mod-slot-prop (p idx)
  (let ((target (instrument-param-knob-mod-target p idx)))
    (if target (instrument-mod-target-source-slot target) false)))

(def instrument-param-knob-mod-depth-prop (p idx)
  (let ((target (instrument-param-knob-mod-target p idx)))
    (if target (instrument-mod-target-depth target) false)))

(def instrument-param-base-value-prop (p)
  (if (and instrument-mods-open (get p :modulatable))
    (instrument-param-base-value p)
    false))

(def instrument-param-base-min-prop (p)
  (if (and instrument-mods-open (get p :modulatable))
    (get p :min)
    false))

(def instrument-param-base-max-prop (p)
  (if (and instrument-mods-open (get p :modulatable))
    (get p :max)
    false))

(def instrument-selected-mod-slot-prop (p)
  (if (and instrument-mods-open (get p :modulatable))
    (instrument-mod-selected-slot)
    false))

(def instrument-param-control-key-mode (p)
  (if (and instrument-mods-open (get p :modulatable))
    "-mod-depth"
    "-base"))

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
              (dict :idx (get target :depth-idx) :control "param")
              v)
            (if (= source-slot 0)
              (do
                (fx-set-instrument-value
                  (dict :idx (get target :source-idx) :control "param")
                  (instrument-mod-selected-slot))
                (fx-set-instrument-value
                  (dict :idx (get target :depth-idx) :control "param")
                  v)))))))
    (fx-set-instrument-value p v)))

(def instrument-toggle-param-modulation (p)
  (if (get p :modulatable)
    (let ((target (instrument-param-selected-mod-target p)))
      (if target
        (if (get target :source-idx)
          (fx-set-instrument-value
            (dict :idx (get target :source-idx) :control "param")
            0)
          (fx-set-instrument-value
            (dict :idx (get target :depth-idx) :control "param")
            0))
        (let ((target (instrument-param-empty-mod-target p)))
          (if target
            (do
              (fx-set-instrument-value
                (dict :idx (get target :source-idx) :control "param")
                (instrument-mod-selected-slot))
              (fx-set-instrument-value
                (dict :idx (get target :depth-idx) :control "param")
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
