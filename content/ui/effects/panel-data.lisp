;; eseq.effects.panel-data — the factory device panels' layout, from the
;; eseq.kinds devices, their params and the drum racks.
;;
;; The panel renderers (instrument-panel, effect-panels, panel-bodies, the
;; param grid, the custom-UI runtime, the built-in effect panels) take a
;; panel as a plain dict: an instrument's (`:synth`, `:mod`, `:sources`, a
;; rack's `:slots`), an effect's (`:params`, `:sources`, its address:
;; `:slot-idx`, `:track-idx`, `:bus-idx`, `:rack-slot`, `:midi-fx`), and a
;; param's (`:name`, `:idx`, `:min`, `:max`, `:options`, …,
;; and `:prm`, the param instance every control binds). This module builds
;; those dicts from the kinds, so the renderers keep one shape for every
;; device family (spec §13.1, the .66 Learned bullet on one renderer over
;; entry dicts). Side-effect free, so any panel module can import it.
;;
;; What a dict reads by value is the panel's structure: the device's
;; descriptor fields (names, ranges, options, placement) and which
;; modulation source settings show (`param.visible`, which follows a source
;; type's value). No dict reads a param's value or text: a control binds
;; its value (`#'prm.value`) and a dropdown reads its label inside its own
;; control (`pc/fx-param-text-value-for`). The *fx* buffer builds each
;; device's dict inside that device's subtree, so a structural change
;; (a source type flipped, by an edit or a p-lock under the playhead)
;; re-renders that panel alone.

(module eseq.effects.panel-data)

(import eseq.kinds :refer (selection buses))
(import eseq.effects.devices :as dv)

(export instrument-panel-of fx-panel-of
        track-instrument-devices track-instrument-panels track-effects
        track-midi-effects bus-effects rack-slot-effect-devices
        current-instrument-panel current-effect-panels)

;; ── Params ──

(def text-or-nil (s) (if (= s "") nil s))

(def enum? (prm) (= prm.type "enum"))

;; The labels of an enum param, else nil.
(def enum-options (prm) (if (enum? prm) prm.options nil))

;; p with prm's UI metadata (the param grid's groups and envelopes, the
;; name the source spells it) and, for a param with no option labels, an
;; unresolved options reference.
(def with-meta (p prm)
  (merge p
    :group (text-or-nil prm.group)
    :env (text-or-nil prm.env)
    :role (text-or-nil prm.role)
    :display-name (text-or-nil prm.display-name)
    :options (or (get p :options) prm.asset-options)))

(def has-lanes? (prm) (> (len prm.mod-targets) 0))

;; An instrument param (a track's or a drum rack slot's): `name` as its
;; list shows it; a boolean is one named enabled or sync with no options.
(def inst-param (prm name)
  (let ((options (enum-options prm)))
    (dict :name name :control "param" :idx prm.index :min prm.min :max prm.max
          :options options
          :boolean (if (and (= options nil) (or (= name "enabled") (= name "sync"))) true nil)
          :prm prm)))

;; A sampler's param: a boolean is its descriptor's.
(def sampler-param (prm name)
  (dict :name name :control "param" :idx prm.index :min prm.min :max prm.max
        :options (enum-options prm)
        :boolean (if (= prm.type "boolean") true nil)
        :prm prm))

;; An effect's param (a track chain, bus or MIDI effect's).
(def fx-param (prm name)
  (dict :name name :idx prm.index :min prm.min :max prm.max
        :options (enum-options prm)
        :boolean (if (= prm.type "boolean") true nil)
        :prm prm))

;; The Delay's sync divisions: its time (param 2) picks one while synced.
(def delay-sync-labels
  '("1/32" "1/16" "1/16t" "1/8" "1/8t" "1/8." "1/4" "1/4t" "1/4." "1/2" "1" "2 bars" "4 bars" "8 bars"))

;; Whether the Delay's sync (param 1, its own value) is on.
(def delay-synced? (d)
  (let ((sync (nth d.params 1)))
    (and sync (> sync.base 0.5))))

;; The Delay's time (param 2) while synced: an index into
;; delay-sync-labels (its dropdown shows the label at the param's value,
;; `pc/fx-param-text-value-for`).
(def delay-synced-time (d prm p)
  (if (and (= d.type "Delay") (= prm.index 2) (delay-synced? d))
    (merge p :options delay-sync-labels :index-options true :min 0 :max 13)
    p))

;; A main param of a track chain or bus effect.
(def effect-main-param (d prm)
  (let ((p (fx-param prm prm.name)))
    (with-meta
      (delay-synced-time d prm
        (merge p
          :unit (if (= prm.type "continuous") (text-or-nil prm.unit) nil)
          :modulatable (if (or (has-lanes? prm) prm.host-modulatable) true nil)))
      prm)))

;; A main param of a drum rack slot's effect.
(def rack-effect-main-param (d prm)
  (let ((p (inst-param prm prm.name)))
    (with-meta
      (merge p
        :unit (if (= prm.type "continuous") (text-or-nil prm.unit) nil)
        :boolean (if (or (get p :boolean) (= prm.type "boolean")) true nil)
        :modulatable (if (has-lanes? prm) true nil)
        :rack-track d.track.index :rack-slot d.container.slot)
      prm)))

;; ── Sections ──

(def params-in (d section)
  (filter (lambda (prm) (= prm.section section)) d.params))

;; The modulation sources' settings (`:sources`, one section per source
;; 1-4): its type param (`:source-param`) and the settings its type uses,
;; each made by `make`.
(def source-sections (d make)
  (let ((settings (filter (lambda (prm) prm.visible) (params-in d "source"))))
    (map (lambda (slot)
           (let ((own (filter (lambda (prm) (= prm.mod-slot slot)) settings)))
             (let ((type-param (first (filter (lambda (prm) (= prm.label "type")) own))))
               (dict :name (str "Mod " slot) :slot slot
                     :source-param (if type-param (make type-param) nil)
                     :params (map make (filter (lambda (prm) (not (= prm.label "type"))) own))))))
         (list 1 2 3 4))))

(def source-names '("Mod 1" "Mod 2" "Mod 3" "Mod 4"))

;; d's fixed modulation sources.
(def modulators-of (d)
  (map (lambda (m) (dict :slot m.slot :label m.label)) d.modulators))

(def tensors-of (d)
  (map (lambda (tz) (dict :idx tz.index :name tz.name :rows tz.rows :cols tz.cols
                          :min tz.min :max tz.max))
       d.tensors))

;; ── Instruments ──

;; A track instrument's base note row (no param: the device's base note).
(def base-note-row (name)
  (dict :name name :control "base-note" :min -48 :max 48))

;; The panel of t's instrument `d` (a synth, a modulator or another
;; custom instrument).
(def synth-panel (t d)
  (dict :type d.type :track t.index :name d.instrument-name
        :display-name d.display-name :meter d.meter
        :synth (cons (base-note-row "base_note")
                 (map (lambda (prm)
                        (with-meta
                          (merge (inst-param prm prm.name)
                            :modulatable (if (has-lanes? prm) true nil))
                          prm))
                      (params-in d "main")))
        :mod (map (lambda (prm) (inst-param prm prm.label)) (params-in d "mod"))
        :tensors (tensors-of d)
        :modulators (modulators-of d)
        :source-names source-names
        :sources (source-sections d (lambda (prm) (inst-param prm prm.label)))))

;; The panel of t's sampler `d`.
(def sampler-panel (t d)
  (let ((synth (cons (base-note-row "base")
                 (map (lambda (prm)
                        (merge (sampler-param prm prm.name)
                          :modulatable (if (has-lanes? prm) true nil)))
                      (params-in d "main")))))
    (dict :type "sampler" :track t.index :meter d.meter
          :params synth :synth synth
          :mod (map (lambda (prm) (sampler-param prm prm.label)) (params-in d "mod"))
          :modulators (modulators-of d)
          :source-names source-names
          :sources (source-sections d (lambda (prm) (sampler-param prm prm.label))))))

;; A drum rack slot instrument's param (its slot's address rides along).
(def rack-slot-param (t sd prm name)
  (merge (inst-param prm name) :rack-track t.index :rack-slot sd.slot))

;; The panel of the instrument in drum rack slot `sd` of t (the rack's
;; selected slot's).
(def rack-slot-instrument-panel (t sd)
  (let ((synth (cons (merge (base-note-row "base_note") :rack-track t.index :rack-slot sd.slot)
                 (map (lambda (prm)
                        (with-meta
                          (merge (rack-slot-param t sd prm prm.name)
                            :modulatable (if (has-lanes? prm) true nil))
                          prm))
                      (params-in sd "main")))))
    (dict :type sd.type :track t.index :rack-track t.index :rack-slot sd.slot
          :meter sd.meter :name sd.name :display-name sd.display-name
          :synth synth :params synth
          :mod (map (lambda (prm) (rack-slot-param t sd prm prm.label)) (params-in sd "mod"))
          :modulators (modulators-of sd)
          :source-names source-names
          :sources (source-sections sd (lambda (prm) (rack-slot-param t sd prm prm.label))))))

;; Whether drum rack slot `sd` holds an instrument (its panel shows, a
;; param-less custom instrument's too; an empty slot shows the drop panel).
(def slot-instrument? (sd)
  (not (or (= sd.type "empty") (= sd.type ""))))

;; The rack strip controls' p-lock targets (the slot rows' right-click
;; menus), in RackSlotParam order.
(def strip-controls '("base-note" "gain" "pan" "max-polyphony" "mute" "solo"))

(def strip-targets (t slot)
  (map (lambda (i) (dict :name (nth strip-controls i) :target "rack-slot-param"
                         :track t.index :slot-idx slot :param-idx i))
       (range 6)))

;; Rack slot `sd` of t's rack, as the rack panel's slot list shows it.
(def rack-slot-row (t sd selected)
  (dict :idx sd.slot :track t.index :type sd.type :name sd.name
        :display-name sd.display-name :enabled sd.enabled
        :selected (= sd.slot selected)
        :param-targets (strip-targets t sd.slot)
        :base-note-min -48 :base-note-max 48
        :gain-min 0 :gain-max 2
        :pan-min -1 :pan-max 1
        :max-polyphony-min 1 :max-polyphony-max 16))

;; The panel of t's drum rack `d`: its slots, the selected slot's
;; instrument.
(def rack-panel (t d)
  (let ((selected selection.rack-slot)
        (slots d.devices))
    (let ((sd (first (filter (lambda (sd) (= sd.slot selected)) slots))))
      (dict :type "rack" :track-id (str t.tid) :track t.index
            :selected-slot selected :name t.name :display-name d.display-name
            :is-rack true :meter d.meter
            :slots (map (lambda (sd) (rack-slot-row t sd selected)) slots)
            :selected-instrument (if (and sd (slot-instrument? sd))
                                   (rack-slot-instrument-panel t sd)
                                   nil)))))

;; The panel of instrument device `d` of track t, or nil: a drum rack's, a
;; sampler's, any other instrument's with params or tensors.
(def instrument-panel-of (t d)
  (cond
    ((= d.type "rack") (rack-panel t d))
    ((= d.type "sampler") (sampler-panel t d))
    ((and (= (len d.params) 0) (= (len d.tensors) 0)) nil)
    (else (synth-panel t d))))

;; Whether instrument device `d` has a panel (`instrument-panel-of`).
(def has-panel? (d)
  (or (= d.type "rack") (= d.type "sampler")
      (> (len d.params) 0) (> (len d.tensors) 0)))

;; t's instrument device when it has a panel, as a list of none or one:
;; the *fx* buffer builds its panel inside its own subtree.
(def track-instrument-devices (t)
  (let ((d (dv/instrument-of t)))
    (if (and d (has-panel? d)) (list d) '())))

;; t's instrument panel, as a list of none or one; none for no track.
(def track-instrument-panels (t)
  (map (lambda (d) (instrument-panel-of t d)) (track-instrument-devices t)))

;; The selected track's instrument panel, or nil (capture fixtures and
;; tests render a custom UI from it).
(def current-instrument-panel ()
  (first (track-instrument-panels selection.track)))

;; ── Effects ──

;; The effect panel's sources and modulators.
(def with-sources (fx d make)
  (merge fx
    :modulators (modulators-of d)
    :source-names source-names
    :sources (source-sections d make)))

;; A source setting of an effect (track chain or bus).
(def effect-source-param (prm) (fx-param prm prm.label))

;; The panel of effect `d` of track t's chain.
(def track-effect-panel (t d)
  (with-sources
    (dict :name d.name :slot-idx d.slot :track-idx t.index :builtin d.builtin
          :meter d.meter
          :params (map (lambda (prm) (effect-main-param d prm)) (params-in d "main")))
    d effect-source-param))

;; A bus effect's source setting: a bus has no voices, so its source type
;; offers no envelope (the dropdown shows the param's text, so dropping an
;; option leaves its label right).
(def bus-source-param (prm)
  (let ((p (effect-source-param prm)))
    (if (and (= prm.label "type") (get p :options))
      (merge p :options (filter (lambda (o) (not (= o "env"))) (get p :options)))
      p)))

;; The panel of bus effect `d` (of the bus at position `bus`).
(def bus-effect-panel (bus d)
  (with-sources
    (dict :name d.name :slot-idx d.slot :bus-idx bus :bus-fx true :builtin d.builtin
          :meter d.meter
          :params (map (lambda (prm) (effect-main-param d prm)) (params-in d "main")))
    d bus-source-param))

;; The panel of MIDI effect `d`: every param.
(def midi-effect-panel (d)
  (dict :name d.name :slot-idx d.slot :midi-fx true
        :params (map (lambda (prm) (fx-param prm prm.name)) d.params)))

;; The panel of effect `d` of drum rack slot `sd` of track t.
(def rack-effect-panel (t sd d)
  (with-sources
    (dict :slot-idx d.slot :name d.name :track-idx t.index :rack-slot sd.slot
          :rack-fx true :meter d.meter :builtin d.builtin
          :params (map (lambda (prm) (rack-effect-main-param d prm)) (params-in d "main")))
    d (lambda (prm) (rack-slot-param t sd prm prm.label))))

;; The effects of drum rack slot `sd` (a device, or nil) with a graph node
;; (a running one), as devices: the rack's selected chain lays each out
;; (`fx-panel-of`) in its own subtree.
(def rack-slot-effect-devices (sd)
  (if sd (filter (lambda (d) (> d.node-id 0)) sd.devices) '()))

;; Whether effect `d` lists any param (the *fx* buffer shows it).
(def lists-params? (d)
  (> (len (params-in d "main")) 0))

;; t's chain effects the *fx* buffer shows, as devices; none for no
;; track.
(def track-effects (t)
  (if t
    (filter (lambda (d) (and (>= d.slot 0) (lists-params? d))) t.devices)
    '()))

;; t's MIDI effects the *fx* buffer shows.
(def track-midi-effects (t)
  (if t (filter (lambda (d) (> (len d.params) 0)) t.midi-devices) '()))

;; The panels of the selected track's chain effects (capture fixtures).
(def current-effect-panels ()
  (map fx-panel-of (track-effects selection.track)))

;; The effects the *fx* buffer shows of bus `b`.
(def bus-effects (b)
  (filter lists-params? b.devices))

;; The panel of effect device `d`: a track chain, MIDI, bus or drum rack
;; slot effect's (by its role), nil for an instrument.
(def fx-panel-of (d)
  (cond
    ((= d.role "effect") (track-effect-panel d.track d))
    ((= d.role "midi-fx") (midi-effect-panel d))
    ((= d.role "bus-effect") (bus-effect-panel d.bus.index d))
    ((= d.role "rack-effect") (rack-effect-panel d.track d.container d))
    (else nil)))
