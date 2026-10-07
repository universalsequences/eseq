;; Parameter value routing, modulation targeting, and wrappers.
;;
;; A control draws one param dict of a panel (an instrument's, or an effect's
;; `fx` dict's): its layout comes from the dict, everything it shows from the
;; eseq.kinds param the dict stands for (eseq.effects.devices/param-of; spec
;; docs/kind-bindings-spec.md §13.1). A widget prop gets a `#'` binding
;; (`#'prm.value`, the knob repaints), a decision a value (`prm.locked`, the
;; control re-renders). Every value is in display units, so a % param reads
;; 0-100; the legacy effect commands the controls send keep their stored
;; units (eseq.effects.devices/param-stored-value converts).
;;
;; The p-lock menu, the process-map arm and the p-lock accent color are this
;; module's view state: `:key ()` singletons below.
(module eseq.effects.param-controls)

(import eseq.kinds :refer (selection macros))
(import eseq.macro-state :as ms :refer (macro-arm rack-armed?))
(import eseq.effects.state :refer (instrument-view key-lock-view effect-mods))
(import eseq.effects.panel-frame :as pf)
(import eseq.effects.devices :as dv)
(import eseq.view-kit :refer (open-menu! rgb-part))

(export instrument-rack-target?
        param-owner-fx
        with-param-owners
        process-map
        process-map-active?
        process-map-port-active?
        process-map-arm-port
        process-map-clear
        process-param-bindable?
        process-send-bindable?
        process-bind-send-target
        process-send-map-wrapper
        param-macro-mapping-active?
        param-macro-structure-key
        instrument-key-lock-has-selection?
        instrument-key-lock-authoring-active?
        instrument-param-base-value
        param-base-value
        fx-set-instrument-value
        fx-set-instrument-option
        custom-ui-option-index
        fx-set-effect-value
        fx-set-effect-stored-value
        effect-param-updates
        fx-toggle-instrument-value
        fx-toggle-effect-value
        fx-param-value
        effect-mods-active?
        effect-mods-track
        fx-has-modulators?
        param-mods-open?
        fx-param-value-for
        fx-param-numeric-value-for
        fx-param-text-value-for
        param-plock-active?
        param-plock-context-menu
        open-target-plock-menu
        open-host-plock-menu
        param-plock-default
        param-plock-color-r
        param-plock-color-g
        param-plock-color-b
        param-plock-text-color
        param-control-min
        param-control-max
        param-control-unit
        percent-scale
        param-set-option
        param-set-control-value
        param-mod-wrapper
        fx-param-numeric-value
        fx-param-on?
        fx-param-on-for?
        instrument-mod-selected-slot
        param-mod-targets
        mod-target-source-slot
        mod-target-depth
        mod-target-depth-idx
        param-knob-mod-slot-prop
        param-knob-mod-depth-prop
        param-base-value-prop
        param-mod-offset
        param-mod-offset-for
        param-effective-value
        param-effective-ratio
        param-mod-scale
        param-mod-scale-for
        param-process-value
        param-process-value-for
        param-process-clamped
        param-process-clamped-for
        param-process-mapped?
        param-process-mapped-for?
        param-process-text-color
        param-base-min-prop
        param-base-max-prop
        param-selected-mod-slot-prop
        param-control-key-mode
        instrument-param-knob-mod-slot-prop
        instrument-param-knob-mod-depth-prop
        instrument-param-base-value-prop
        instrument-param-base-min-prop
        instrument-param-base-max-prop
        instrument-selected-mod-slot-prop
        instrument-param-control-key-mode
        instrument-param-control-min
        instrument-param-control-max
        instrument-set-param-control-value
        instrument-param-mod-wrapper)

;; Migration aliases (module spec §10). This is the most-depended-on file in
;; effects/: the effect panels (plus Rust test harnesses in
;; src/ui/state_values/tests.rs) call these names by their flat spelling, and
;; bare callers cannot see qualified names. Every public name keeps its
;; spelling — like eseq.effects.state, this hub mixes several prefix families
;; (`param-`, `process-map-`, `instrument-`, `fx-`) and stripping any of them
;; would collide — so every alias below is an identity alias. They are deleted
;; as the caller files convert. `%`-private helpers get no alias.

(def instrument-rack-target? (p)
  (not (= (get p :rack-track) nil)))

;; ── Process map arm ──

;; A process port armed to bind a param (step process lanes): its track
;; position and process instance id, the port's name and the kind of target
;; it takes ("" any). Track -1 while nothing is armed.
(def-kind process-map
  :key ()
  :state ((track -1)
          (instance-id 0)
          (port "")
          (target-kind "")))

(def process-map-active? ()
  (and (>= process-map.track 0)
       (> process-map.instance-id 0)
       (not (= process-map.port ""))))

(def process-map-port-active? (track slot port)
  (and (process-map-active?)
       (= process-map.track track)
       (= process-map.instance-id (get slot :instance-id))
       (= process-map.port (get port :name))))

(def process-map-arm-port (track slot port)
  (if (process-map-port-active? track slot port)
    (process-map-clear)
    (do
      (ms/clear-mapping-arm)
      (ms/rack-clear-mapping-arm)
      (set! instrument-view.mods-open false)
      (set! effect-mods.open false)
      (set! process-map.track track)
      (set! process-map.instance-id (get slot :instance-id))
      (set! process-map.port (get port :name))
      (set! process-map.target-kind (if (get port :target-kind) (get port :target-kind) ""))
      (eseq.seq-panels/seq-show-fx-lower-panel))))

(def process-map-clear ()
  (if (or (not (= process-map.track -1))
          (not (= process-map.instance-id 0))
          (not (= process-map.port ""))
          (not (= process-map.target-kind "")))
    (do
      (set! process-map.track -1)
      (set! process-map.instance-id 0)
      (set! process-map.port "")
      (set! process-map.target-kind ""))
    false))

(add-hook "macro-mapping-arm-enter-hook" "param-controls"
  (lambda ()
    (do
      (process-map-clear)
      (ms/rack-clear-mapping-arm)
      (set! instrument-view.mods-open false)
      (set! effect-mods.open false))))

(def process-map-target-map (fx p)
  (if (not (fx-param-has-idx? p))
    false
    (if fx
      (if (get fx :midi-fx)
        (dict :kind "midi-fx" :slot-idx (get fx :slot-idx)
              :fx (get fx :name) :param-idx (get p :idx) :param (get p :name))
        (if (get fx :bus-fx)
          false
          (dict :kind "effect" :slot-idx (get fx :slot-idx)
                :effect (get fx :name) :param-idx (get p :idx) :param (get p :name))))
      (if (instrument-rack-target? p)
        false
        (dict :kind "instrument" :param-idx (get p :idx) :param (get p :name))))))

(def process-map-target-compatible? (target)
  (let ((kind (get target :kind))
        (wanted process-map.target-kind))
    (if (= wanted "")
      true
      (if (= wanted "device-param")
        (or (= kind "instrument") (= kind "effect") (= kind "midi-fx") (= kind "bus-send"))
        (if (= wanted "instrument-param")
          (= kind "instrument")
          (if (= wanted "effect-param")
            (= kind "effect")
            (if (= wanted "midi-fx-param")
              (= kind "midi-fx")
              false)))))))

(def process-param-bindable? (fx p)
  (let ((target (process-map-target-map fx p)))
    (if target
      (process-map-target-compatible? target)
      false)))

(def process-bind-param-target (fx p)
  (let ((target (process-map-target-map fx p)))
    (if (and target (process-map-target-compatible? target))
      (do
        (seq-bind-process-port process-map.track process-map.instance-id process-map.port target)
        (process-map-clear))
      nil)))

(def process-param-map-bg (fx p)
  (if (and (process-map-active?) (process-param-bindable? fx p))
    (rgba 0.93 0.65 0.16 0.25)
    :transparent))

;; Mixer / track-panel bus sends as process targets. A process writes on
;; its own track, so only that track's send knobs light up while mapping:
;; clicking another strip's send would silently bind this track's send.
;; `send` names the bus as a dict (:bus-id :bus-idx :name): the track
;; panel's send entries, the mixer's `send-target` of a send instance.
(def process-send-target-map (send)
  (dict :kind "bus-send" :bus-id (get send :bus-id) :bus-idx (get send :bus-idx)
        :param (get send :name)))

(def process-send-bindable? (track send)
  (and (process-map-active?)
       (= track process-map.track)
       (process-map-target-compatible? (process-send-target-map send))))

(def process-bind-send-target (track send)
  (if (process-send-bindable? track send)
    (do
      (seq-bind-process-port process-map.track process-map.instance-id process-map.port
        (process-send-target-map send))
      (process-map-clear))
    nil))

;; Same box treatment as the instrument-knob map wrapper above: the box
;; captures the pointer so a click binds instead of starting a knob drag.
(def process-send-map-wrapper (track send key body)
  (if (process-send-bindable? track send)
    (subtree :key (str key "-process-map")
      (box :background-color (rgba 0.93 0.65 0.16 0.25)
        :debug-name "process-send-map-wrapper"
        :corner-radius 8
        :border-width 1
        :padding 0.08
        :capture-pointer true
        :on-click (lambda (info) (process-bind-send-target track send))
        body))
    body))

;; ── Macro mapping ──

(def param-macro-mapping-active? ()
  (or (and macro-arm.open (>= macro-arm.mid 0))
      (rack-armed?)))

;; The map-rack-macro-param command's address of a drum rack slot's param
;; (its instrument's or one of its effects'), false for any other param.
(def rack-macro-target-map (fx p)
  (if fx
    (if (get fx :rack-fx)
      (dict :kind "rack-slot-effect" :rack-slot (get fx :rack-slot)
        :effect-slot (get fx :slot-idx) :param-idx (get p :idx) :param (get p :name)
        :min (get p :min) :max (get p :max))
      false)
    (if (instrument-rack-target? p)
      (dict :kind "rack-slot-instrument" :rack-slot (get p :rack-slot)
        :param-idx (get p :idx) :param (get p :name) :min (get p :min) :max (get p :max))
      false)))

;; The current drum rack's macros (its instrument device's; empty for any
;; other track).
(def rack-macros ()
  (let ((d (dv/instrument-of selection.track)))
    (if d d.macros '())))

(def rack-macro-selected ()
  (first (filter (lambda (rm) (= rm.index macro-arm.rack-index)) (rack-macros))))

;; The mapping of macro m (a project or rack macro) driving prm, or nil. A
;; project mapping whose target is gone drives nothing.
(def mapping-onto (m prm)
  (if (and m prm)
    (first (filter (lambda (mm) (and (not mm.suspended) (= mm.target prm))) m.mappings))
    nil))

(def rack-macro-mapping-for (fx p)
  (mapping-onto (rack-macro-selected) (dv/param-of fx p)))

(def rack-macro-owner-for (fx p)
  (let ((prm (dv/param-of fx p)))
    (when prm (first (filter (lambda (rm) (mapping-onto rm prm)) (rack-macros))))))

(def param-macro-bindable? (fx p)
  (if (rack-armed?)
    (rack-macro-target-map fx p)
    (and (get p :modulatable) (process-map-target-map fx p))))

(def project-macro-selected ()
  (first (filter (lambda (m) (= m.mid macro-arm.mid)) (macros))))

;; Names the selected project macro's mappings, so the *fx* buffer's map
;; mode rebuilds when they change.
(def param-macro-structure-key ()
  (let ((m (project-macro-selected)))
    (str "fx-macro-map-" macro-arm.mid "-"
         (if m
           (map (lambda (mm) (list mm.index mm.label mm.suspended mm.min mm.max)) m.mappings)
           '()))))

(def param-macro-mapping-for (fx p)
  (mapping-onto (project-macro-selected) (dv/param-of fx p)))

(def param-macro-owner-for (fx p)
  (let ((prm (dv/param-of fx p)))
    (when prm (first (filter (lambda (m) (mapping-onto m prm)) (macros))))))

(def param-macro-owner-mapping-for (fx p)
  (let ((prm (dv/param-of fx p)))
    (when prm
      (mapping-onto (first (filter (lambda (m) (some-mapping? m prm)) (macros))) prm))))

;; Some rack or project macro drives p's param.
(def param-macro-owned? (fx p)
  (let ((prm (dv/param-of fx p)))
    (and prm
         (not (empty? (filter (lambda (m) (some-mapping? m prm))
                        (append (rack-macros) (macros))))))))

(def some-mapping? (m prm)
  (not (= (mapping-onto m prm) nil)))

(def param-macro-bg (fx p)
  (if (and (param-macro-mapping-active?) (param-macro-bindable? fx p))
    (if (if (rack-armed?)
          (rack-macro-mapping-for fx p)
          (param-macro-mapping-for fx p))
      (rgba 0.18 0.45 0.142 0.98)
      (rgba 0.18 0.35 0.242 0.9))
    :transparent))

(def param-macro-map (fx p)
  (if (rack-armed?)
    (let ((target (rack-macro-target-map fx p)) (mapped (rack-macro-mapping-for fx p)))
      (if mapped
        (host-command "unmap-rack-macro-param"
          (dict :track (dv/current-track-index) :id macro-arm.rack-index
            :mapping-idx mapped.index))
        (if target (host-command "map-rack-macro-param"
          (merge target :id macro-arm.rack-index :track (dv/current-track-index))) false)))
    (let ((target (process-map-target-map fx p)))
      (if (and target
               (not (rack-macro-owner-for fx p))
               (not (param-macro-owner-mapping-for fx p)))
        (host-command "macro-map-param"
          (merge target :id macro-arm.mid :track (dv/current-track-index)))
        false))))

;; ── Key locks (the instrument panel's keys tab) ──

(def instrument-target-param-dict (source-p idx)
  (if (instrument-rack-target? source-p)
    (dict :idx idx :control "param"
          :rack-track (get source-p :rack-track)
          :rack-slot (get source-p :rack-slot))
    (dict :idx idx :control "param")))

(def instrument-keys-active? ()
  (= instrument-view.tab 1))

(def instrument-key-lock-has-selection? ()
  (> (len key-lock-view.notes) 0))

(def instrument-key-lock-authoring-active? ()
  (and (instrument-keys-active?) (instrument-key-lock-has-selection?)))

;; The first selected key, or nil (note 0 is a key: compare with nil).
(def instrument-selected-key-note ()
  (first key-lock-view.notes))

;; p's key lock on `note`: its `(note value)` row, or nil.
(def instrument-param-key-lock-row (p note)
  (let ((prm (dv/param-of false p)))
    (if prm (first (filter (lambda (row) (= (first row) note)) prm.key-locks)) nil)))

(def instrument-param-key-lock-active? (p)
  (let ((note (instrument-selected-key-note)))
    (if (= note nil)
      false
      (if (instrument-param-key-lock-row p note) true false))))

;; ── Values ──

;; The effect a param dict belongs to, for the one-argument forms
;; (fx-param-value, param-mod-offset, …): the owner a custom UI's scoped param
;; (eseq.effects.custom-ui-runtime) or a built-in effect panel's param
;; (with-param-owners) carries, else false (an instrument param). Effect
;; panels may also pass their fx to the -for forms.
(def param-owner-fx (p)
  (let ((owner (get p :custom-ui-owner)))
    (if owner (get owner :fx) false)))

;; fx with each of its params resolved once (`:prm`, eseq.effects.devices'
;; with-prm) and owned by it (param-owner-fx), for a panel that hands bare
;; param dicts to the one-argument forms. The owner is fx's address only, so
;; a param never carries its siblings.
(def with-param-owners (fx)
  (let ((owner (dict :fx (dict :name (get fx :name) :slot-idx (get fx :slot-idx)
                               :track-idx (get fx :track-idx) :bus-idx (get fx :bus-idx)
                               :rack-slot (get fx :rack-slot) :rack-fx (get fx :rack-fx)
                               :bus-fx (get fx :bus-fx) :midi-fx (get fx :midi-fx)
                               :target-node-id (get fx :target-node-id)))))
    (merge fx :params
      (map (lambda (p) (merge (dv/with-prm fx p) :custom-ui-owner owner)) (get fx :params)))))

(def fx-param-has-idx? (p)
  (not (= (get p :idx) nil)))

;; The value param dict p shows, as a binding: its param's value (the base
;; note control: the instrument's base note), else the dict's own value.
(def param-base-value (fx p)
  (if (dv/base-note-param? p)
    (let ((d (dv/param-device fx p)))
      (if d #'d.base-note-display (get p :value)))
    (let ((prm (dv/param-of fx p)))
      (if prm #'prm.value (get p :value)))))

(def instrument-param-base-value (p)
  (param-base-value (param-owner-fx p) p))

(def instrument-param-key-lock-value (p)
  (let ((note (instrument-selected-key-note)))
    (if (= note nil)
      (param-base-value false p)
      (let ((row (instrument-param-key-lock-row p note)))
        (if row (nth row 1) (param-base-value false p))))))

(def fx-set-instrument-value (p v)
  (do
    (pf/fx-clear-selected-effect)
    (let ((rack-track (get p :rack-track))
          (rack-slot (get p :rack-slot)))
      (if (instrument-rack-target? p)
        (if (dv/base-note-param? p)
          (host-command (if (seq-has-selection?) "set-rack-slot-param-plock" "set-rack-slot-base-note")
            (dict :track rack-track :slot rack-slot :param "base-note" :value v))
          (host-command
            (if (seq-has-selection?) "set-rack-slot-instrument-plock" "set-rack-slot-instrument-param")
            (dict :track rack-track :slot rack-slot :param-idx (get p :idx) :value v)))
        (if (dv/base-note-param? p)
          (host-command "set-instrument-base-note" (dict :value v))
          (host-command
            (if (instrument-key-lock-authoring-active?)
              "set-instrument-key-lock-multi"
              (if (seq-has-selection?) "set-instrument-plock" "set-instrument-param"))
            (dict :param-idx (get p :idx) :value v :notes key-lock-view.notes)))))))

(def fx-set-instrument-option (p label)
  (do
    (pf/fx-clear-selected-effect)
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
          (dict :param-idx (get p :idx) :label label :notes key-lock-view.notes))))))

(def custom-ui-option-index (options label)
  (nth (filter |idx| (= (nth options idx) label) (range (len options))) 0))

;; Set fx's param p to v (display units), or p-lock the selected steps to it.
;; A param not published is left alone: its stored units are unknown.
(def fx-set-effect-value (fx p v)
  (let ((prm (dv/param-of fx p)))
    (when prm (fx-set-effect-stored-value fx p (dv/param-stored-value fx prm v)))))

;; fx's batch update setting p to v (display units), in the stored units the
;; batch commands take; nil for a param not published.
(def effect-param-update (fx p v)
  (let ((prm (dv/param-of fx p)))
    (when prm (dict :param-idx (get p :idx) :value (dv/param-stored-value fx prm v)))))

;; The batch updates of `pairs` ((p v) lists), skipping unpublished params.
(def effect-param-updates (fx pairs)
  (filter (lambda (u) (not (= u nil)))
    (map (lambda (pair) (effect-param-update fx (nth pair 0) (nth pair 1))) pairs)))

;; Set fx's param p to `stored`, already in the effect commands' stored units
;; (a preset table's), or p-lock the selected steps to it.
(def fx-set-effect-stored-value (fx p stored)
  (do
    (pf/fx-clear-selected-effect)
    (if (get fx :rack-fx)
      (host-command (if (seq-has-selection?) "set-rack-slot-effect-plock" "set-rack-slot-effect-param")
        (dict :track (get fx :track-idx)
              :rack-slot (get fx :rack-slot)
              :effect-slot (get fx :slot-idx)
              :param (get p :idx)
              :value stored))
    (if (get fx :bus-fx)
      (host-command (if (seq-has-selection?) "set-bus-effect-plock" "set-bus-effect-param")
        (dict :bus (get fx :bus-idx) :slot-idx (get fx :slot-idx)
              :param-idx (get p :idx) :value stored))
    (if (get fx :midi-fx)
      (host-command
        (if (seq-has-selection?) "set-midi-fx-plock" "set-midi-fx-param")
        (dict :slot-idx (get fx :slot-idx) :param-idx (get p :idx) :value stored))
      (if (seq-has-selection?)
        (seq-set-effect-plock (get fx :slot-idx) (get p :idx) stored (get fx :target-node-id))
        (host-command "set-effect-param"
          (dict :slot-idx (get fx :slot-idx)
                :target-node-id (get fx :target-node-id)
                :param-idx (get p :idx) :value stored))))))))

(def fx-toggle-instrument-value (p)
  (do
    (pf/fx-clear-selected-effect)
    (let ((rack-track (get p :rack-track))
          (rack-slot (get p :rack-slot)))
      (if (instrument-rack-target? p)
        (host-command
          (if (seq-has-selection?) "toggle-rack-slot-instrument-plock" "toggle-rack-slot-instrument-param")
          (dict :track rack-track :slot rack-slot :param-idx (get p :idx)))
        (if (instrument-key-lock-authoring-active?)
          (host-command "set-instrument-key-lock-multi"
            (dict :param-idx (get p :idx)
                  :notes key-lock-view.notes
                  :value (if (fx-param-on? p) 0 1)))
          (host-command "toggle-instrument-param"
            (dict :param-idx (get p :idx))))))))

(def fx-toggle-effect-value (fx p)
  (do
    (pf/fx-clear-selected-effect)
    (if (get fx :rack-fx)
      (host-command (if (seq-has-selection?) "set-rack-slot-effect-plock" "set-rack-slot-effect-param")
        (dict :track (get fx :track-idx)
              :rack-slot (get fx :rack-slot)
              :effect-slot (get fx :slot-idx)
              :param (get p :idx)
              :value (if (fx-param-on-for? fx p) 0 1)))
      (host-command "toggle-effect-param"
        (dict :bus (get fx :bus-idx)
              :bus-fx (get fx :bus-fx)
              :midi-fx (get fx :midi-fx)
              :slot-idx (get fx :slot-idx)
              :param-idx (get p :idx))))))

;; See fx-param-value-for (p's effect: param-owner-fx).
(def fx-param-value (p)
  (fx-param-value-for (param-owner-fx p) p))

;; The effect-mods track of fx: its track position, -1 for a bus effect or a
;; dict naming none (a MIDI effect's: the current track's).
(def effect-mods-track (fx)
  (let ((track (get fx :track-idx)))
    (if (or (get fx :bus-fx) (= track nil)) -1 track)))

(def effect-mods-active? (fx)
  (and fx
       effect-mods.open
       (= effect-mods.chain (pf/fx-effect-chain-kind fx))
       (= effect-mods.track (effect-mods-track fx))
       (= effect-mods.slot (get fx :slot-idx))
       (= effect-mods.rack-slot (if (get fx :rack-fx) (get fx :rack-slot) -1))
       (= effect-mods.bus (if (get fx :bus-fx) (get fx :bus-idx) -1))))

(def fx-has-modulators? (fx)
  (and fx (> (len (get fx :sources)) 0)))

(def param-mods-open? (fx)
  (if fx (effect-mods-active? fx) instrument-view.mods-open))

(def instrument-mod-selected-slot ()
  (if (> instrument-view.mod-slot 0) instrument-view.mod-slot 1))

(def param-mod-selected-slot (fx)
  (if fx
    (if (> effect-mods.mod-slot 0) effect-mods.mod-slot 1)
    (instrument-mod-selected-slot)))

;; ── Modulation lanes ──

;; The modulation lanes onto p's param (mod-target instances), or none.
(def param-mod-targets (fx p)
  (let ((prm (dv/param-of fx p)))
    (if prm prm.mod-targets '())))

;; The modulation source (1-4, 0 none) lane mt reads: its source param's
;; value, else its fixed slot.
(def mod-target-source-slot (mt)
  (let ((source mt.source))
    (if source source.value mt.slot)))

;; Lane mt's depth, as a binding (the depth param's display units).
(def mod-target-depth (mt)
  (let ((depth mt.depth))
    (if depth #'depth.value 0)))

(def mod-target-depth-idx (mt)
  (let ((depth mt.depth)) (if depth depth.index nil)))

(def mod-target-source-idx (mt)
  (let ((source mt.source)) (if source source.index nil)))

(def param-selected-mod-target (fx p)
  (first
    (filter (lambda (mt) (= (mod-target-source-slot mt) (param-mod-selected-slot fx)))
      (param-mod-targets fx p))))

(def param-empty-mod-target (fx p)
  (first
    (filter (lambda (mt) (and mt.source (= (mod-target-source-slot mt) 0)))
      (param-mod-targets fx p))))

(def param-control-mod-target (fx p)
  (let ((selected-target (param-selected-mod-target fx p)))
    (if selected-target
      selected-target
      (let ((empty-target (param-empty-mod-target fx p)))
        (if empty-target
          empty-target
          (first (param-mod-targets fx p)))))))

;; The value p's control shows: on the keys tab the selected key's lock;
;; while modulation is open (and p modulatable) the depth of the lane its
;; knob edits; else the param's value. A binding where the value is live.
(def fx-param-value-for (fx p)
  (if (and (not fx) (instrument-keys-active?) (fx-param-has-idx? p))
    (instrument-param-key-lock-value p)
    (if (and (param-mods-open? fx) (get p :modulatable))
      (let ((mt (param-control-mod-target fx p)))
        (if mt (mod-target-depth mt) 0))
      (param-base-value fx p))))

;; Options are indexed by the param's value, which is not guaranteed to be in
;; range: a synced Delay time, for instance, stores milliseconds while its
;; sync options list holds a dozen divisions. `nth` past the end returns nil
;; and the dropdown then renders "nil", so clamp the index the way the old
;; Rust builder did.
(def fx-param-option-at (options value)
  (nth options (clamp (round value) 0 (- (len options) 1))))

;; The option label p's dropdown shows: on the keys tab the selected key's
;; lock's; else its param's text, not an index into the dict's :options by
;; value (they may leave options out: an effect's source types lack env);
;; the option at its value for a param with no labels of its own. The
;; mods-open depth branch must not leak into the label, so it reads the
;; param's own value.
(def fx-param-text-value-for (fx p)
  (let ((options (get p :options)))
    (if (and options (not fx) (instrument-keys-active?))
      (fx-param-option-at options (fx-param-value-for fx p))
      (let ((prm (dv/param-of fx p)))
        (if (and prm (not (= prm.text "")))
          prm.text
          (if (and prm options)
            (fx-param-option-at options prm.value)
            (get p :text-value)))))))

(def param-plock-row-target (fx)
  (if fx
    (if (get fx :rack-fx) "rack-effect"
      (if (get fx :midi-fx) "midi-fx" "effect"))
    "instrument"))

;; ── P-locks ──

;; A p-lock supplies the value p's param shows at the displayed step.
(def param-locked? (fx p)
  (let ((prm (dv/param-of fx p)))
    (if prm prm.locked false)))

(def param-plock-active? (fx p)
  (if (and (not fx) (instrument-keys-active?))
    (instrument-param-key-lock-active? p)
    (and (not (param-mods-open? fx))
         (param-locked? fx p))))

;; Automation presence (bead eseq-yr6w): this param carries a p-lock on SOME
;; step of the pattern, not necessarily the selected one. Drives the corner dot,
;; never the value readout.
(def param-plock-any? (fx p)
  (let ((prm (dv/param-of fx p)))
    (if prm prm.has-locks false)))

;; --- Right-click "clear p-locks" menu (bead eseq-1gy6) ---------------------
;; One menu shared by every param control: open at the right-click's grid
;; point, on the key tuple of the param whose knob was right-clicked (the
;; clear-param-plocks command's address). The menu itself
;; (param-plock-context-menu) is rendered by each buffer that hosts knobs — an
;; overlay has to live in the active tile. More than one buffer hosts it
;; (*fx* and *mixer*), and the anchor is tile-relative, so the state also
;; records which host opened it: without that, every visible host draws a
;; copy at the same offset in its own tile, and the stray copy steals the
;; clicks meant for the real one.
;;
;; A param with no p-locks has nothing to offer, so its right-click is a no-op
;; and no empty menu opens. The wrappers only bind :on-right-click on the
;; branches that already draw a box, which is every branch that can carry the
;; presence dot; the bare-body fast path stays free of a wrapper.
(def-kind plock-menu
  :key ()
  :state ((open false)
          (at :point :default nil)
          (host "fx")
          (target :any :default nil)))

;; Key tuple the host command clears by: the real slot indices of the
;; param's storage.
(def param-plock-menu-target (fx p)
  (dict :track (dv/current-track-index) :target (param-plock-row-target fx)
        :slot-idx (if fx (get fx :slot-idx) 0)
        :rack-slot (if (and fx (get fx :rack-fx)) (get fx :rack-slot) 0)
        :param-idx (get p :idx)))

(def open-param-plock-menu (event fx p)
  (open-target-plock-menu event (param-plock-menu-target fx p) (param-plock-any? fx p)))

(def open-target-plock-menu (event target has-locks)
  (open-host-plock-menu event "fx" target has-locks))

(def open-host-plock-menu (event host target has-locks)
  (if has-locks
    (do
      (set! plock-menu.host host)
      (set! plock-menu.target target)
      (open-menu! plock-menu event)
      true)
    false))

(def close-param-plock-menu ()
  (set! plock-menu.open false))

(def param-plock-selection-label (count)
  (if (= count 1)
    "Clear p-locks on 1 selected step"
    (str "Clear p-locks on " count " selected steps")))

;; The selection is read only while the menu is open, so the hosting buffer
;; does not follow it the rest of the time.
(def param-plock-menu-actions ()
  (if plock-menu.open
    (let ((count (len selection.steps)))
      (if (> count 0)
        (list (dict :id "all" :label "Clear p-locks")
              (dict :id "selected" :label (param-plock-selection-label count)))
        (list (dict :id "all" :label "Clear p-locks"))))
    (list)))

(def clear-param-plocks (scope)
  (let ((target plock-menu.target))
    (do
      (close-param-plock-menu)
      (if target
        (host-command "clear-param-plocks"
          (dict :track (get target :track)
                :target (get target :target)
                :slot-idx (get target :slot-idx)
                :rack-slot (get target :rack-slot)
                :param-idx (get target :param-idx)
                :scope scope))
        false))))

(def param-plock-menu-open-in? (host)
  (and plock-menu.open (= plock-menu.host host)))

(def param-plock-context-menu (host)
  (context-menu :is-open (param-plock-menu-open-in? host)
    :anchor plock-menu.at
    :on-close (lambda () (close-param-plock-menu))
    (each (param-plock-menu-actions) |action|
      (menu-item (get action :label)
        :key (str "param-plock-menu-" (get action :id))
        :on-select (lambda (event) (clear-param-plocks (get action :id)))))))

;; Live print latch (bead eseq-4seq): this param is the one being held while
;; play+record writes its value onto passing steps (param.printing is false
;; unless the transport plays and records).
(def param-print-latched? (fx p)
  (let ((prm (dv/param-of fx p)))
    (if prm prm.printing false)))

;; The value a locked control shows as its unlocked one: the param's own
;; value.
(def param-plock-default (fx p)
  (let ((prm (dv/param-of fx p)))
    (if (and prm prm.locked) #'prm.base (fx-param-value-for fx p))))

;; The p-lock accent: the color of the step variant the selected step plays,
;; else the default lock color. One effect (below) follows the current
;; track's variants; controls bind the components and only repaint.
(def-kind plock-color
  :key ()
  :state ((r 0.27058825)
          (g 0.78431374)
          (b 0.8627451)))

(def param-plock-color-r () #'plock-color.r)
(def param-plock-color-g () #'plock-color.g)
(def param-plock-color-b () #'plock-color.b)

(def sync-plock-color! ()
  (let ((t selection.track))
    (let ((v (if t (first (filter (lambda (v) v.current) t.variants)) nil)))
      (do
        (set! plock-color.r (if v (rgb-part v.color 0) 0.27058825))
        (set! plock-color.g (if v (rgb-part v.color 1) 0.78431374))
        (set! plock-color.b (if v (rgb-part v.color 2) 0.8627451))))))

;; Runs as a dedicated non-visual effect buffer: a plain (effect ...) treats
;; its result as the source buffer's widget tree, which would clobber the
;; buffer that loaded this file.
(effect-buffer "*plock-color-sync*"
  (do (sync-plock-color!) nil))

(def param-plock-text-color (fx p)
  (if (param-plock-active? fx p)
    (rgba plock-color.r plock-color.g plock-color.b 1.0)
    :dim))

;; ── Control ranges and setters ──

(def param-control-min (fx p)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (let ((mt (param-control-mod-target fx p)))
      (if mt mt.depth-min -1))
    (get p :min)))

(def param-control-max (fx p)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (let ((mt (param-control-mod-target fx p)))
      (if mt mt.depth-max 1))
    (get p :max)))

;; Unit shown on a param's control. In the mods tab the knob edits a *depth*,
;; whose units are the destination's own modulation units and need not match
;; the base param's: the built-in Filter's cutoff reads 20..20000 Hz but scales
;; exponentially, so its depth is +/-4 *octaves*. Showing the depth's unit is
;; what makes that legible instead of looking like a 4 Hz sweep. Outside the
;; mods tab the param's own unit is used, and `false` when neither declares one
;; so the display is unchanged.
(def param-control-unit (fx p)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (let ((mt (param-control-mod-target fx p)))
      (if (and mt (not (= mt.unit ""))) mt.unit false))
    (let ((unit (get p :unit)))
      (if unit unit false))))

;; The :value-scale a percent control shows p with. Its own value: a 0-1
;; fraction (x 100) unless its unit is % (display units, x 1). A lane depth
;; while modulation is open: x 100 only for a % lane whose depth param is a
;; plain 0-1 fraction (no % of its own, so not already x 100).
(def percent-scale (fx p)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (let ((mt (param-control-mod-target fx p)))
      (if (and mt (= mt.unit "%") (not (depth-percent? mt))) 100 1))
    (if (= (get p :unit) "%") 1 100)))

(def depth-percent? (mt)
  (let ((depth mt.depth)) (and depth depth.percent)))

(def param-set-option (fx p label)
  (if fx
    (do
      (pf/fx-clear-selected-effect)
      (if (get fx :rack-fx)
        (host-command
          (if (seq-has-selection?) "set-rack-slot-effect-plock-option" "set-rack-slot-effect-param-option")
          (dict :track (get fx :track-idx)
                :rack-slot (get fx :rack-slot)
                :effect-slot (get fx :slot-idx)
                :param (get p :idx)
                :label label))
        (host-command
          (if (get fx :bus-fx)
            (if (seq-has-selection?) "set-bus-effect-plock-option" "set-bus-effect-param-option")
            (if (get fx :midi-fx)
              (if (seq-has-selection?) "set-midi-fx-plock-option" "set-midi-fx-param-option")
              (if (seq-has-selection?) "set-effect-plock-option" "set-effect-param-option")))
          (dict :bus (get fx :bus-idx) :slot-idx (get fx :slot-idx)
                :target-node-id (get fx :target-node-id)
                :param-idx (get p :idx) :label label))))
    (fx-set-instrument-option p label)))

;; Set param `idx` of p's device (a lane's source or depth) to v.
(def set-device-param-at (fx p idx v)
  (if fx
    (fx-set-effect-value fx (dict :idx idx :control "param") v)
    (fx-set-instrument-value (instrument-target-param-dict p idx) v)))

(def param-set-control-value (fx p v)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (let ((mt (param-control-mod-target fx p)))
      (if mt
        (let ((source-slot (mod-target-source-slot mt))
              (selected-slot (param-mod-selected-slot fx)))
          (if (= source-slot selected-slot)
            (set-device-param-at fx p (mod-target-depth-idx mt) v)
            (if (= source-slot 0)
              (do
                (set-device-param-at fx p (mod-target-source-idx mt) selected-slot)
                (set-device-param-at fx p (mod-target-depth-idx mt) v))
              nil)))
        nil))
    (if fx (fx-set-effect-value fx p v) (fx-set-instrument-value p v))))

(def param-toggle-modulation (fx p)
  (if (get p :modulatable)
    (let ((mt (param-selected-mod-target fx p))
          (selected-slot (param-mod-selected-slot fx)))
      (if mt
        (if mt.source
          (set-device-param-at fx p (mod-target-source-idx mt) 0)
          (set-device-param-at fx p (mod-target-depth-idx mt) 0))
        (let ((empty (param-empty-mod-target fx p)))
          (if empty
            (do
              (set-device-param-at fx p (mod-target-source-idx empty) selected-slot)
              (set-device-param-at fx p (mod-target-depth-idx empty) 0))
            nil))))
    nil))

;; ── Wrappers ──

(def param-mod-bg (fx p)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (rgba 0.03 0.20 0.35 0.94)
    :transparent))

(def param-mod-border (fx p)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (rgba 0.18 0.48 0.95 0.84)
    :transparent))

(def param-mod-wrapper (fx p key body)
  (if (param-macro-mapping-active?)
    (if (param-macro-bindable? fx p)
      (let ((mapped (if (rack-armed?)
              (rack-macro-mapping-for fx p) (param-macro-mapping-for fx p)))
          (owner (param-macro-owned? fx p)))
        (subtree :key (str key "-macro-map")
          (box :background-color (param-macro-bg fx p)
            :debug-name (if owner "macro-param-owned-wrapper" "macro-param-map-wrapper")
            :corner-radius 8
            :border-width (if mapped 2 1)
            :border-color (if mapped (rgba 0.32 1.0 0.55 1.0)
              (if owner (rgba 0.18 0.85 0.42 0.62) (rgba 0.18 0.85 0.42 0.75)))
            :macro-owned (if owner 1 0)
            :padding 0.08
            :capture-pointer true
            :on-click (lambda (info) (param-macro-map fx p))
            :on-right-click (lambda (event) (open-param-plock-menu event fx p))
            body)))
      body)
    (if (and (not (param-mods-open? fx)) (param-macro-owned? fx p))
      (subtree :key (str key "-macro-owned")
        (box :debug-name "macro-param-owned-wrapper"
          :background-color :transparent
          :corner-radius 8
          :border-width 0
          :macro-owned 1
          :plock-any (if (param-plock-any? fx p) 1 0)
          :capture-pointer true
          :on-click (lambda (info) false)
          :on-right-click (lambda (event) (open-param-plock-menu event fx p))
          body))
      (if (process-map-active?)
        (if (process-param-bindable? fx p)
          (subtree :key (str key "-process-map")
            (box :background-color (process-param-map-bg fx p)
              :debug-name "process-param-map-wrapper"
              :corner-radius 8
              :border-width 1
              :padding 0.08
              :capture-pointer true
              :on-click (lambda (info) (process-bind-param-target fx p))
              :on-right-click (lambda (event) (open-param-plock-menu event fx p))
              body))
          body)
        ;; Print overlay outranks the mods-tab box: while the knob is being
        ;; recorded onto passing steps that is the only thing worth saying
        ;; about it. Same box treatment as macro map mode, p-lock accent.
        (if (param-print-latched? fx p)
          (subtree :key (str key "-plock-print")
            (box :debug-name "param-print-wrapper"
              :background-color (rgba 0.04 0.20 0.26 0.92)
              :corner-radius 8
              :border-width 1
              :border-color :widget-plock-accent
              :plock-any (if (param-plock-any? fx p) 1 0)
              :padding 0
              :on-right-click (lambda (event) (open-param-plock-menu event fx p))
              body))
          (if (and (param-mods-open? fx) (get p :modulatable))
            (subtree :key key
              (box :background-color (param-mod-bg fx p)
                :border-color (param-mod-border fx p)
                :corner-radius 8
                :border-width 1
                :plock-any (if (param-plock-any? fx p) 1 0)
                :padding 0.08
                :on-double-click (lambda (info) (param-toggle-modulation fx p))
                :on-right-click (lambda (event) (open-param-plock-menu event fx p))
                body))
            ;; Neutral state: only pay for a wrapper box when there is a dot
            ;; to draw on it. That box is also the right-click target: a param
            ;; with no p-locks has nothing to clear, so the fast path needs no
            ;; handler either.
            (if (param-plock-any? fx p)
              (subtree :key (str key "-plock-any")
                (box :debug-name "param-plock-any-wrapper"
                  :background-color :transparent
                  :corner-radius 8
                  :border-width 0
                  :plock-any 1
                  :on-right-click (lambda (event) (open-param-plock-menu event fx p))
                  body))
              body)))))))

(def fx-param-numeric-value (p)
  (fx-param-numeric-value-for (param-owner-fx p) p))

;; The value as a number, read now (0 for none).
(def fx-param-numeric-value-for (fx p)
  (number-now (fx-param-value-for fx p)))

;; v read now (a binding reads its field, spec §8) as a number, 0 for nil.
(def number-now (v)
  (if (= v nil) 0 (+ v 0)))

(def fx-param-on? (p)
  (> (fx-param-numeric-value p) 0.5))

(def fx-param-on-for? (fx p)
  (> (fx-param-numeric-value-for fx p) 0.5))

(def param-knob-mod-target (fx p idx)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (nth (param-mod-targets fx p) idx)
    false))

(def param-knob-mod-slot-prop (fx p idx)
  (let ((mt (param-knob-mod-target fx p idx)))
    (if mt (mod-target-source-slot mt) false)))

(def param-knob-mod-depth-prop (fx p idx)
  (let ((mt (param-knob-mod-target fx p idx)))
    (if mt (mod-target-depth mt) false)))

;; Live modulation offset for a param (eseq-hpc): how far modulation currently
;; pushes it from its base, in the base's own units. The host samples the
;; modulator node at meter rate; knobs draw their live dot at that
;; displacement from the base they are already showing.
;;
;; An offset rather than the absolute effective value on purpose. The base moves
;; the instant a knob is dragged while the sampler only runs at meter rate, so
;; an absolute value trails the drag and flashes a dot beside a knob nothing is
;; modulating; an offset rides along with the base instead, and an unmodulated
;; param's offset is exactly 0. Read-only telemetry either way — nothing here
;; writes back into widget state, so dragging a modulated knob still edits the
;; base value. An unmodulated param's offset is 0, which draws no dot;
;; `false` for a control with no param (nothing to show).
(def param-mod-offset-for (fx p)
  (let ((prm (dv/param-of fx p)))
    (if prm #'prm.mod-offset false)))

;; The absolute effective value, for curve visualizers: one binding straight
;; into a widget prop (a computed `base + offset` would not follow the bound
;; value). The param's value when it is not modulated.
(def param-effective-value-for (fx p)
  (let ((prm (dv/param-of fx p)))
    (if prm #'prm.mod-value (get p :value))))

;; The effective value as a 0-1 fraction where p is a fraction % param
;; (param.mod-ratio: its display value / 100), for visualizers drawn on the
;; stored scale; else as param-effective-value-for. A binding.
(def param-effective-ratio-for (fx p)
  (let ((prm (dv/param-of fx p)))
    (if prm #'prm.mod-ratio (get p :value))))

;; The multiplicative form of the same displacement, for destinations whose
;; modulation mode is exponential (the built-in Filter's cutoff, whose depth is
;; in octaves). `base + offset` only composes with a *moving* base when the mode
;; is additive: a +2-octave lane sampled at 1 kHz reads a 3 kHz offset, and a
;; knob dragged to 8 kHz before the next 50 ms tick would draw its dot at
;; 11 kHz instead of 32 kHz. The scale is `2^octaves` — exactly 1.0 for
;; additive destinations — and the knob prefers `base * scale` whenever it is
;; not 1.0. `false` for a control with no param.
(def param-mod-scale-for (fx p)
  (let ((prm (dv/param-of fx p)))
    (if prm #'prm.mod-scale false)))

;; Process effective value (eseq-p1kg). When an enabled step process writes
;; to this instrument param through a bound OUT port, the value the instrument
;; actually received (base, or the step's p-lock, plus the port value, clamped
;; to the range) in the param's display units. The knob draws it as a second
;; dot in the process accent; the number picker as a strip anchored at the
;; base. Read-only: the control keeps editing the base. `false` while not
;; mapped, so a stale write never draws on an unmapped control.
(def param-process-mapped-for? (fx p)
  (let ((prm (dv/param-of fx p)))
    (if prm prm.process-mapped false)))

(def param-process-value-for (fx p)
  (let ((prm (dv/param-of fx p)))
    (if (and prm prm.process-mapped) #'prm.process-value false)))

;; True when the last process write hit the range end and was clamped.
(def param-process-clamped-for (fx p)
  (let ((prm (dv/param-of fx p)))
    (if (and prm prm.process-mapped) #'prm.process-clamped 0)))

;; The one-argument forms resolve p through its owner (param-owner-fx).
(def param-mod-offset (p) (param-mod-offset-for (param-owner-fx p) p))
(def param-mod-scale (p) (param-mod-scale-for (param-owner-fx p) p))
(def param-effective-value (p) (param-effective-value-for (param-owner-fx p) p))
(def param-effective-ratio (p) (param-effective-ratio-for (param-owner-fx p) p))
(def param-process-value (p) (param-process-value-for (param-owner-fx p) p))
(def param-process-clamped (p) (param-process-clamped-for (param-owner-fx p) p))
(def param-process-mapped? (p) (param-process-mapped-for? (param-owner-fx p) p))

;; P-lock colour wins (the step override is the more specific state); a
;; process-mapped param otherwise reads in the process accent so the user can
;; tell it is being generatively driven even while the offset is zero.
(def param-process-text-color (fx p)
  (if (param-plock-active? fx p)
    (rgba plock-color.r plock-color.g plock-color.b 1.0)
    (if (param-process-mapped-for? fx p) :process-lane-accent :dim)))

(def param-base-value-prop (fx p)
  (if (and (param-mods-open? fx) (get p :modulatable))
    (param-base-value fx p)
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

(def instrument-param-control-min (p)
  (param-control-min false p))

(def instrument-param-control-max (p)
  (param-control-max false p))

(def instrument-set-param-control-value (p v)
  (param-set-control-value false p v))

(def instrument-param-mod-bg (p)
  (if (and instrument-view.mods-open (get p :modulatable))
    (rgba 0.18 0.48 0.95 0.24)
    :transparent))

;; The instrument panel's own wrapper look (fainter macro borders, an
;; unbordered mods box) over the same states as param-mod-wrapper.
(def instrument-param-mod-wrapper (p key body)
  (if (param-macro-mapping-active?)
    (if (param-macro-bindable? false p)
      (let ((mapped (if (rack-armed?)
                      (rack-macro-mapping-for false p) (param-macro-mapping-for false p)))
            (owner (param-macro-owned? false p)))
        (subtree :key (str key "-macro-map")
          (box :background-color (param-macro-bg false p)
               :debug-name (if owner "macro-param-owned-wrapper" "macro-param-map-wrapper")
               :corner-radius 8
               :border-width (if mapped 2 1)
               :border-color (if mapped (rgba 0.32 1.0 0.55 1.0)
                 (if owner (rgba 0.18 0.85 0.42 0.22) (rgba 0.18 0.85 0.42 0.55)))
               :macro-owned (if owner 1 0)
               :padding 0.08
               :capture-pointer true
               :on-click (lambda (info) (param-macro-map false p))
               :on-right-click (lambda (event) (open-param-plock-menu event false p))
            body)))
      body)
  (if (and (not instrument-view.mods-open) (param-macro-owned? false p))
    (subtree :key (str key "-macro-owned")
      (box :debug-name "macro-param-owned-wrapper"
           :background-color :transparent
           :corner-radius 8
           :border-width 0
           :macro-owned 1
           :plock-any (if (param-plock-any? false p) 1 0)
           :capture-pointer true
           :on-click (lambda (info) false)
           :on-right-click (lambda (event) (open-param-plock-menu event false p))
        body))
  (if (process-map-active?)
    (if (process-param-bindable? false p)
      (subtree :key (str key "-process-map")
        (box :background-color (process-param-map-bg false p)
             :debug-name "process-param-map-wrapper"
             :corner-radius 8
             :border-width 1
             :padding 0.0
             :capture-pointer true
             :on-click (lambda (info) (process-bind-param-target false p))
             :on-right-click (lambda (event) (open-param-plock-menu event false p))
          body))
      body)
    ;; See param-mod-wrapper: printing outranks the mods-tab box.
    (if (param-print-latched? false p)
      (subtree :key (str key "-plock-print")
        (box :debug-name "param-print-wrapper"
             :background-color (rgba 0.04 0.20 0.26 0.92)
             :corner-radius 8
             :border-width 1
             :border-color :widget-plock-accent
             :plock-any (if (param-plock-any? false p) 1 0)
             :padding 0
             :on-right-click (lambda (event) (open-param-plock-menu event false p))
          body))
      (if (and instrument-view.mods-open (get p :modulatable))
        (subtree :key key
          (box :background-color (instrument-param-mod-bg p)
               :corner-radius 8
               :border-width 1
               :plock-any (if (param-plock-any? false p) 1 0)
               :padding 0.08
               :on-double-click (lambda (info) (param-toggle-modulation false p))
               :on-right-click (lambda (event) (open-param-plock-menu event false p))
            body))
        (if (param-plock-any? false p)
          (subtree :key (str key "-plock-any")
            (box :debug-name "param-plock-any-wrapper"
                 :background-color :transparent
                 :corner-radius 8
                 :border-width 0
                 :plock-any 1
                 :on-right-click (lambda (event) (open-param-plock-menu event false p))
              body))
          body)))))))
