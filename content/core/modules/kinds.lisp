;; eseq.kinds — the host kinds: what a view can be built from
;; (docs/kind-bindings-spec.md §3.4, §4, §9, §14).
;;
;; Each kind here is a projection of the sequencer's own state. The host
;; registers the instances (tracks, scenes, banks, buses, groups, devices
;; (a track's chain, MIDI effects and drum rack slots, a bus's effects),
;; sends, clips, cells, scene spans, drum rack pads, rack clips and grooves,
;; tensors, p-lock variants, project and drum rack macros and their mappings;
;; steps on first read of `t.steps`, params (with their modulation lanes) on
;; first read of `d.params`),
;; pushes their `:host` fields,
;; and checks at startup that it publishes exactly the fields declared below
;; (crates/sequencer/src/ui/host_kinds/). A view imports what it uses:
;;
;;   (import eseq.kinds :refer (track tracks transport scenes banks selection launch!))
;;
;; Read a field by value (`t.name`, re-renders the reader) or bind it
;; (`#'t.volume`, repaints only). Writable fields carry `:set`: `(set! t.volume
;; 0.5)`, `(toggle! t.muted)`, `(toggle! s.active)`, `(set! selection.track t)`,
;; `(set! t.swing 56)`, `(set! t.output nil)`, `(set! tn.morph 0.5)` (tn = t.tuning).
;; The host computes a field only while something observes it (a reader or a
;; held `#'`); reading an unobserved one asks the host for its value.

(module eseq.kinds)

(export track scene bank bus group transport selection project master engine
        song region
        tracks scenes banks buses groups routes macros
        launch! clone-scene! delete-scene! step-preset!
        device-param lock-param! unlock-param!
        set-tensor-cell! stamp-variant! stamp-key-variant!
        lock-none lock-seq lock-variant
        reset-tuning! justify-tuning! randomize-tuning! stretch-tuning!
        set-bar-transpose! mod-in-level
        mute-group-options accum-mode-options tuning-root-options tuning-mode-options
        voice-priority-options mono-trigger-options swing-resolution-options
        roll-rate-options
        launch-cell! select-region! clear-region! take-none take-governed take-latched
        pad-role-options groove-scale-options
        trigger-pad! launch-rack-clip! silence-rack! save-rack-clip-as! delete-rack-clip!
        convert-rack-to-clips! use-library-groove! apply-groove-to-all-clips! extract-groove!
        duplicate-groove! delete-groove! save-groove-to-library!)

;; Short fixed option lists (the host checks they match its own). The lists
;; the host owns (scales, step sync resolutions, accumulators, track outputs)
;; are `project` fields: project.fts-options, project.sync-options, ….
(def mute-group-options '("Off" "1" "2" "3" "4" "5" "6" "7" "8"))
(def accum-mode-options '("rtz" "clip" "rvtz" "rvbp"))
(def tuning-root-options '("C" "C#" "D" "D#" "E" "F" "F#" "G" "G#" "A" "A#" "B"))
(def tuning-mode-options '("Snap" "Map"))
(def voice-priority-options '("Last" "High" "Low"))
(def mono-trigger-options '("retrig" "legato"))
(def swing-resolution-options '("1/16" "1/8" "1/4" "1/2"))
(def roll-rate-options '("4" "4T" "8" "8T" "16" "16T" "32" "32T"))
;; A drum rack pad's explicit role keys (pad.role; "" is Standard: the role
;; the standard layout infers from the note), and a groove's time scales.
(def pad-role-options '("kick" "snare" "rim" "clap" "closed-hat" "pedal-hat" "open-hat"
                        "tom-low" "tom-mid" "tom-high" "crash" "ride" "shaker" "perc"))
(def groove-scale-options '(0.5 1 2))

;; ── :set functions (thin wrappers over the existing natives) ──

;; The setters are absolute: the host compares with the model when the
;; command lands, so two `set!`s (or `toggle!`s) in one frame never undo
;; each other through a cell that has not caught up yet.
(def set-track-volume (t v) (seq-set-track-volume t.index (max 0 (min 1 v))))
(def set-track-muted (t v) (seq-set-track-mute t.index v))
(def set-track-armed (t v) (seq-set-record-arm t.index v))
(def set-step-active (s v) (seq-set-track-step s.track.index s.index v))
(def set-transport-playing (tr v) (seq-set-playing v))
(def set-transport-recording (tr v) (seq-set-recording v))
(def select-track (sel t) (if t (seq-set-track t.index) nil))
(def set-track-pan (t v) (seq-set-track-pan t.index (max -1 (min 1 v))))
(def set-track-soloed (t v) (seq-set-track-solo t.index v))
(def set-track-collapsed (t v) (seq-set-track-collapsed t.index v))
(def step-param-setter (param)
  (lambda (s v) (seq-set-track-step-param s.track.index s.index param v)))
;; The track's own send level (never a p-lock), addressed by bus id so a bus
;; reorder before the command lands cannot retarget it.
(def set-send-amount (s v)
  (host-command "set-track-send-base"
    (dict :track s.track.index :bus-id s.bus.bid :amount (max 0 (min 1 v)))))
(def set-bus-volume (b v) (seq-set-bus-volume b.index (max 0 (min 1 v))))
(def set-bus-muted (b v) (seq-set-bus-mute b.index v))
(def set-bus-soloed (b v) (seq-set-bus-solo b.index v))
(def set-group-collapsed (g v) (seq-set-group-collapsed g.gid v))
(def set-transport-bpm (tr v) (seq-set-bpm v))
(def set-transport-metronome (tr v) (host-command "set-metronome" v))
(def set-transport-roll-mode (tr v) (host-command "set-roll-mode" v))
(def set-transport-record-quantize (tr v) (host-command "set-record-quantize" v))
(def set-master-recording (m v) (seq-set-master-recording v))
;; Track settings (the track panel): absolute, addressed by the track's stable
;; id so a reorder before the command lands cannot retarget it. One undo entry
;; per set! (a drag view's set!s while the pointer is held join one entry).
;; Values (spec §14.2c): a string field takes one of its labels
;; (case-insensitive; its current value always works), a number field a
;; number in range, a bool field a bool; anything else is an error.
(def track-setting (setting)
  (lambda (t v)
    (host-command "set-track-setting" (dict :track-id t.tid :setting setting :value v))))
;; The output: a bus instance (the main mix bus for main), nil for sends only.
(def set-track-output (t b)
  (host-command "set-track-setting"
    (dict :track-id t.tid :setting "output" :bus-id (if b b.bid nil))))
;; true makes t the mixer's delete target (unless it already is one of
;; them); false takes t out of it (from a multi-track target, the others
;; stay).
(def set-track-delete-target (t v) (seq-set-track-delete-target t.index v))
;; A bus's output, addressed by bus ids.
(def set-bus-output (b dest)
  (if dest
    (host-command "set-bus-output" (dict :bus-id b.bid :destination-id dest.bid))
    nil))
(def route-target (r)
  (dict :source r.source.index :dest-kind (if r.dest "track" "bus")
        :dest (if r.dest r.dest.index r.dest-bus.bid) :input (- r.input 1)))
(def set-route-selected (r v)
  (if v
    (seq-set-delete-target :mod-route (route-target r))
    (if (seq-delete-target? :mod-route (route-target r)) (seq-clear-delete-target) nil)))
;; The step cursor: makes s's track the current one and moves the grid's
;; cursor to s, as a click on the step does.
(def set-cursor-step (sel s)
  (if s
    (host-command "set-cursor-step" (dict :track-id s.track.tid :step s.index))
    nil))
(def set-transport-roll-rate (tr v) (seq-set-roll-rate v))
;; The scale editor (`tuning` and its `degree`s), by the track's stable id.
(def tuning-edit (tn op v)
  (host-command "set-tuning" (dict :track-id tn.track.tid :op op :value v)))
(def tuning-setter (op) (lambda (tn v) (tuning-edit tn op v)))
(def degree-setter (op)
  (lambda (d v)
    (host-command "set-tuning"
      (dict :track-id d.tuning.track.tid :op op :degree d.index :value v))))
;; The arrangement (song, clips, the track's pattern cells). Clips are
;; addressed by their stable clip id, tracks by their stable track id, cells
;; by (track id, pattern id), so an edit or a reorder before the command lands
;; cannot retarget them. Script drags (spec §14.2d): while the pointer is
;; down, every clip start / end and song end set! (of any number of clips)
;; joins ONE undo entry, and each frame rebuilds the arrangement from where
;; the drag started with every target set so far, so a clip dragged across
;; another only occludes it where it ends up. With the pointer up each set!
;; is its own entry (an end past a take's end grows the take, as on the
;; timeline). Values (spec §14.2c): beats are finite numbers >= 0 (an end
;; after its start), flags are bools; a rejected arrangement edit shows in
;; song.edit-error.
(def song-setter (field)
  (lambda (sg v) (host-command "set-song" (dict :field field :value v))))
(def set-song-bound-clip (sg c)
  (host-command "set-song" (dict :field "bound-clip" :clip-id (if c c.cid nil))))
(def set-track-latched (t v)
  (host-command "set-song" (dict :field "latched" :track-id t.tid :value v)))
(def clip-setter (field)
  (lambda (c v) (host-command "set-clip" (dict :clip-id c.cid :field field :value v))))
;; Plays cell (a pattern of the clip's own track) instead; nil is an error.
(def set-clip-cell (c cl)
  (host-command "set-clip"
    (dict :clip-id c.cid :field "cell"
          :track-id (if cl cl.track.tid nil) :pattern-id (if cl cl.pid nil))))
(def set-cell-selected (c v)
  (host-command "set-cell"
    (dict :track-id c.track.tid :pattern-id c.pid :field "selected" :value v)))
;; A device by stable ids: its track's id (a chain device, a MIDI effect, a
;; drum rack slot or one of its effects) or its bus's (a bus effect), and its
;; did, so a reorder before the command lands cannot retarget it.
(def device-target (d)
  (if d.bus
    (dict :bus-id d.bus.bid :device d.did)
    (dict :track-id d.track.tid :device d.did)))
;; A device param's own value (never a p-lock), in display units.
(def set-param-base (p v)
  (host-command "set-device-param"
    (merge (device-target p.device) :param-idx p.index :value v)))
;; A device's own fields (set-device): a rack slot's voices, the delete
;; target.
(def device-setter (field)
  (lambda (d v)
    (host-command "set-device" (merge (device-target d) :field field :value v))))

;; Macros (spec §14.2g): a project macro by its id, a drum rack's macro by its
;; rack's device and its index, a mapping by its macro and its position.
(def macro-setter (field)
  (lambda (m v) (host-command "set-macro" (dict :macro-id m.mid :field field :value v))))
(def rack-macro-setter (field)
  (lambda (rm v)
    (host-command "set-rack-macro"
      (merge (device-target rm.device) :macro rm.index :field field :value v))))
(def mapping-target (mm)
  (if mm.macro
    (dict :macro-id mm.macro.mid :mapping mm.index)
    (merge (device-target mm.rack-macro.device) :macro mm.rack-macro.index :mapping mm.index)))
(def mapping-setter (field)
  (lambda (mm v)
    (host-command "set-macro-mapping" (merge (mapping-target mm) :field field :value v))))

;; Drum racks (spec §14.2e). Pads are addressed by their rack's group id and
;; their member track's stable id, rack clips by (group id, clip id), grooves
;; by (group id, clip id; 0 for the rack's own), pool grooves by their id, all
;; resolved when the command lands. A groove amount's set!s while the pointer
;; is down join one undo entry; everything else is its own entry.
(def pad-setter (field)
  (lambda (p v)
    (host-command "set-pad"
      (dict :group-id p.group.gid :track-id p.track.tid :field field :value v))))
(def set-group-armed (g v) (seq-set-rack-armed g.gid v))
(def rack-clip-setter (field)
  (lambda (rc v)
    (host-command "set-rack-clip"
      (dict :group-id rc.group.gid :clip-id rc.cid :field field :value v))))
(def groove-clip-id (gr) (if gr.clip gr.clip.cid 0))
(def groove-setter (field)
  (lambda (gr v)
    (host-command "set-groove"
      (dict :group-id gr.group.gid :clip-id (groove-clip-id gr) :field field :value v))))
(def set-groove-pool-groove (gr pg)
  ((groove-setter "pool-groove") gr (if pg pg.groove-id nil)))
(def pad-groove-setter (field)
  (lambda (pq v)
    (host-command "set-groove"
      (dict :group-id pq.groove.group.gid :clip-id (groove-clip-id pq.groove)
            :track-id pq.pad.track.tid :field field :value v))))
(def set-pool-groove-name (pg v)
  (host-command "set-pool-groove" (dict :groove-id pg.groove-id :field "name" :value v)))

;; ── Kinds ──

;; One step slot of a track: (nth t.steps 5). Positional: a scene switch
;; changes no instance, only the values (spec D2).
(def-kind step
  :key (track index)
  :host ((index    :int    :doc "Step index on its track, from 0")
         (track    track   :doc "The track the step belongs to")
         (active   :bool   :set set-step-active :doc "The step triggers")
         (playing  :bool   :doc "The playhead is on this step while the transport runs")
         (selected :bool   :doc "Selected for editing (the current track's steps only)")
         (held     :bool   :doc "Inside an active step's duration (that step included)")
         ;; Step parameters, in the host's units (seq-set-track-step-param).
         (velocity    :number :set (step-param-setter :velocity))
         (duration    :number :set (step-param-setter :duration) :doc "Length in steps")
         (transpose   :number :set (step-param-setter :transpose))
         (delay       :number :set (step-param-setter :delay))
         (retrig      :number :set (step-param-setter :retrig))
         (retrig-rate :number :set (step-param-setter :retrig-rate))
         (pan         :number :set (step-param-setter :pan))
         (sync        :number :set (step-param-setter :sync))
         (aux-a       :number :set (step-param-setter :aux-a))
         ;; P-lock display (any family: device params, sends, step params, …).
         (plocked       :bool :doc "Some p-lock lands on this step")
         (lock-kind     :int  :doc "lock-none, lock-seq (sequencer-only locks) or lock-variant (a p-lock variant)")
         (variant-color :rgb  :doc "The step's p-lock variant color (gray for sequencer-only locks)")))

;; One track's send to one bus (not the main mix): (nth t.sends 0).
(def-kind send
  :key (track bus)
  :host ((track   track :doc "The sending track")
         (bus     bus   :doc "The receiving bus")
         (amount  :number :range (0 1) :set set-send-amount
                  :doc "The track's send level (the base; setting it never p-locks)")
         (display :number :range (0 1)
                  :doc "The level shown: on the current track the p-lock at the selected (or playing) step, else amount")
         (locked  :bool   :doc "display comes from a p-lock")
         (has-locks :bool :doc "Some step of the track's pattern locks this send")))

;; One parameter of a device: (nth d.params 3), or (device-param d "cutoff").
;; Params belong to their device: a reorder keeps them; another effect or
;; instrument in the device (a different descriptor) replaces them, and the
;; old ones go stale. Every value, the range and the setters speak display
;; units, the same for instrument and effect params: a % param reads 0-100.
(def-kind param
  :key (device index)
  :host ((device  device  :doc "The device the param belongs to")
         (index   :int    :doc "Parameter index in the device's descriptor, from 0")
         (name    :string)
         (min     :number :doc "Range, in display units")
         (max     :number)
         (default :number :doc "The descriptor's default")
         (type    :string :doc "continuous, enum or boolean (value 0 or 1)")
         (options (list-of :string) :doc "Labels of an enum param, by value; empty otherwise")
         (unit    :string :doc "Display unit of a continuous param (Hz, ms, %); may be empty")
         (value   :number :doc "The value shown: a selected neural neuron's output override (overridden), else on the current track the p-lock at the selected (or playing) step, else the base under any engaged macro")
         (base    :number :set set-param-base
                  :doc "The device's own value (setting it never p-locks; see lock-param!). Set values are clamped; enum and boolean ones rounded (true/false work)")
         (locked  :bool   :doc "A p-lock supplies the value at the displayed step (even while overridden shows another)")
         (overridden :bool :doc "value shows a selected neural neuron's output override (step editing), not the p-lock or base")
         (has-locks :bool :doc "Some step of the track's pattern locks this param")
         (text    :string :doc "The option label value selects (on/off for a boolean); empty for continuous params")
         (printing :bool  :doc "Held under a live print latch while playing and recording")
         ;; Panel placement (spec §14.2g).
         (label   :string :doc "The name the panel shows: a mod param without its mod prefix, a modulation source's setting by its role (type, rate, attack, …)")
         (section :string :doc "main, mod (a modulation lane's own param), source (a modulation source's setting, of source mod-slot) or hidden (host plumbing)")
         (mod-slot :int   :doc "The modulation source (1-4) a source param sets; 0 otherwise")
         (visible :bool   :doc "Shown: false for a hidden param and for a source param its source's type does not use")
         ;; Modulation and process display.
         (mod-targets (list-of mod-target) :doc "The modulation lanes onto this param")
         (mod-offset :number :doc "How far modulation moves value now (display units); 0 while unmodulated or not sampled")
         (mod-value  :number :doc "Where modulation moves value now; value while unmodulated")
         (mod-scale  :number :doc "An exponential destination's modulation ratio (mod-value / value); 1 otherwise")
         (process-mapped :bool :doc "An enabled process slot of the track writes this instrument param")
         (process-value :number :doc "The value a process last wrote here (display units); value when none has")
         (process-clamped :bool :doc "That write hit the end of the param's range")
         (key-locks (list-of (list-of :number)) :doc "An instrument param's key locks, (note value) per locked key, ascending (display units)")))

;; One modulation lane onto a param: (nth p.mod-targets 0).
(def-kind mod-target
  :key (param index)
  :host ((param  param :doc "The modulated param")
         (index  :int)
         (source param :doc "The param picking the lane's modulation source; nil for a fixed source (slot)")
         (slot   :int  :doc "The fixed modulation source (1-4) when source is nil")
         (depth  param :doc "The lane's depth param")
         (depth-min :number :doc "The depth's range, in the depth param's display units")
         (depth-max :number)
         (unit   :string :doc "The depth's display unit; may be empty")))

;; A device's tensor (a table of cells): (nth d.tensors 0).
(def-kind tensor
  :key (device index)
  :host ((device device)
         (index  :int)
         (name   :string)
         (rows   :int)
         (cols   :int)
         (min    :number :doc "The cells' range")
         (max    :number)
         (values (list-of :number) :doc "The cells shown, row by row: the p-lock at the displayed step, else base")
         (base   (list-of :number) :doc "The device's own cells (set-tensor-cell! sets one)")
         (locked :bool :doc "values come from a p-lock")))

;; A p-lock variant: a set of locks several steps share (t.variants), or keys
;; share (an instrument's d.variants). Keyed by its label while it exists.
(def-kind variant
  :key ((track device) vid)
  :host ((track   track  :doc "The track whose variant it is")
         (device  device :doc "The instrument of a key-lock variant; nil for a step variant")
         (label   :string :doc "A, B, …")
         (name    :string :doc "The name shown: its own name, else label")
         (count   :int    :doc "How many params it locks")
         (color   :rgb)
         (current :bool   :doc "A step variant the selected step plays")
         (notes   (list-of :int) :doc "The keys a key-lock variant is stamped on")))

;; A project macro: (macros), project.macros.
(def-kind macro
  :key (index)
  :host ((index :int)
         (mid   :int    :doc "The host's stable macro id")
         (script-key :string :doc "A script's key for it (macro-ensure), optional: empty when none (a rack macro's stable-key is always set)")
         (name  :string :set (macro-setter "name"))
         (type  :string :doc "mapped or scene")
         (value :number :range (0 1) :set (macro-setter "value")
                :doc "A performance control: setting it is not an undo entry")
         (mappings (list-of macro-mapping))
         (target-scene scene :doc "A scene macro's scene; nil for a mapped one")
         (morph-params :bool)
         (steal-patterns :bool)
         (quantize :string :doc "A scene macro's steal quantization (off, sixteenth, bar); empty for a mapped one")))

;; A drum rack's macro: (nth d.macros 0) of the rack's instrument device.
(def-kind rack-macro
  :key (device index)
  :host ((device device :doc "The drum rack's instrument device")
         (index  :int    :doc "0-7")
         (stable-key :string :doc "Its stable id (macro_1, …), always set; a project macro's script-key is the optional script key instead")
         (name   :string :set (rack-macro-setter "name"))
         (value  :number :range (0 1) :doc "The value shown: the p-lock at the displayed step, else base under any engaged project macro")
         (base   :number :range (0 1) :set (rack-macro-setter "value")
                 :doc "The macro's own value (setting it never p-locks)")
         (locked :bool :doc "value comes from a p-lock")
         (has-locks :bool :doc "Some step of the track's pattern locks it")
         (mappings (list-of macro-mapping))))

;; What one macro drives: (nth m.mappings 0). A mapping has no stable id, so
;; its handle is positional: deleting a mapping retargets the handles of the
;; mappings after it to the mapping now at their position.
(def-kind macro-mapping
  :key ((macro rack-macro) index)
  :host ((macro      macro      :doc "Its project macro; nil for a rack macro's")
         (rack-macro rack-macro :doc "Its rack macro; nil for a project macro's")
         (index  :int)
         (target param  :doc "The param it drives; nil when that is no device param (or gone)")
         (label  :string :doc "What it drives, as the panel names it")
         (min    :number :set (mapping-setter "min") :doc "The target's value at macro 0 (display units)")
         (max    :number :set (mapping-setter "max") :doc "… and at macro 1")
         (curve  :string :set (mapping-setter "curve") :doc "linear, exp, log or log-domain (a project mapping's; a rack mapping's is linear, exp or log)")
         (suspended :bool :doc "A project mapping that drives nothing now (its target is gone)")))

;; One device: of a track's chain (t.devices: the instrument, slot -1, then
;; its effects), a track's MIDI effects (t.midi-devices), a drum rack's slots
;; (the rack instrument's devices) and each slot's effects (the slot's
;; devices), or a bus's effects (b.devices). Keyed under its track or bus by
;; its stable id: a reorder keeps the instance, only slot moves.
(def-kind device
  :key ((track bus) did)
  :host ((track   track   :doc "The track that holds the device; nil for a bus effect")
         (bus     bus     :doc "The bus whose effect it is; nil for a track's device")
         (slot    :int    :doc "Position in its own chain: -1 for the instrument, else the effect, MIDI effect, rack slot or bus effect slot")
         (did     :int    :doc "The host's stable device id under its track or bus: 0 for the instrument, else the device's instance id")
         (role    :string :doc "instrument, effect, midi-fx, rack-slot, rack-effect or bus-effect")
         (type    :string :doc "What the device is: the instrument type (synth, sampler, rack, …) or the effect (Filter, Reverb, …)")
         (name    :string :doc "Device name")
         (enabled :bool   :doc "False while bypassed (a rack slot: switched off)")
         (params  (list-of param) :doc "The device's parameters, in descriptor order")
         (playhead :number :doc "A sampler's playing position in seconds, 0 when idle or not a sampler")
         (devices (list-of device) :doc "The devices it holds: a drum rack's slots, a rack slot's effects; empty otherwise")
         (container device :doc "The device whose devices holds this one (a rack slot's rack, a rack slot effect's slot), or nil")
         (voices  :int :set (device-setter "voices")
                  :doc "A drum rack slot's voices, 1 (mono) to 16 (the setter checks; no declared range: 0 reads for any other device, which takes no set!)")
         (delete-target :bool :set (device-setter "delete-target")
                  :doc "The delete target (Backspace deletes it). Track effects are targets only on the current track; an instrument never")
         ;; Panel extras (spec §14.2g).
         (base-note :number :range (-48 48) :set (device-setter "base-note")
                    :doc "A track instrument's base note offset, in semitones; 0 for any other device (which takes no set!)")
         (mod-phases (list-of :number) :doc "Each modulation source's (1-4) cycle position; -1 when it has none or nothing samples it")
         (tensors (list-of tensor) :doc "The device's tensors (tables of cells)")
         (key-locked-notes (list-of :int) :doc "A track instrument's keys holding a key lock, ascending")
         (variants (list-of variant) :doc "A track instrument's key-lock variants (stamp-key-variant!)")
         (macros (list-of rack-macro) :doc "A drum rack's macros (on its instrument device); empty otherwise")))

(def-kind track
  :key (index)
  :host ((index     :int    :doc "Position in the track list, from 0")
         (tid       :int    :doc "The host's stable track id")
         (name      :string)
         (color     :rgb    :doc "Display color")
         (volume    :number :range (0 1) :set set-track-volume)
         (peak      :number :range (0 1) :doc "Output meter level")
         (muted     :bool   :set set-track-muted)
         (audible   :bool   :doc "Heard: neither muted nor silenced by another track's solo")
         (armed     :bool   :set set-track-armed :doc "Record-armed")
         (selected  :bool   :doc "The current track (set selection.track)")
         (preset    :string :doc "Loaded preset name; empty when none")
         (num-steps :int    :doc "Pattern length in steps")
         (steps     (list-of step))
         (devices   (list-of device) :doc "The chain: the instrument, then its effects")
         (midi-devices (list-of device) :doc "The MIDI effects, in chain order")
         (pan       :number :range (-1 1) :set set-track-pan)
         (soloed    :bool   :set set-track-soloed)
         (collapsed :bool   :set set-track-collapsed :doc "Lane collapsed in the sequencer")
         (playhead  :int    :doc "The playing step, -1 while stopped")
         (timebase  :string :doc "Step timebase: 1/16, 1/8T, …")
         (instrument-type :string :doc "Instrument kind: synth, sampler, rack, …")
         (rack      :bool   :doc "A drum rack track")
         (group     group   :doc "The group holding the track, or nil")
         (sends     (list-of send))
         ;; Track settings (the track panel). Setters take what the field reads.
         (poly      :bool   :set (track-setting "poly")
                    :doc "Polyphonic (the track's own flag; a drum rack's voices are per slot)")
         (max-polyphony :int :range (1 16) :set (track-setting "max-polyphony") :doc "Voices")
         (gate      :bool   :set (track-setting "gate") :doc "Notes last their step duration")
         (supports-mono-trigger :bool :doc "The instrument honours voice-priority and mono-trigger")
         (voice-priority :string :set (track-setting "voice-priority") :doc "One of voice-priority-options")
         (mono-trigger :string :set (track-setting "mono-trigger") :doc "One of mono-trigger-options")
         (mute-group :int   :range (0 8) :set (track-setting "mute-group")
                    :doc "0 for none, else the group; (nth mute-group-options g) is its label")
         (swing     :number :range (50 75) :set (track-setting "swing")
                    :doc "The track's own swing percent (a step's swing p-lock never shows here)")
         (swing-resolution :string :set (track-setting "swing-resolution")
                    :doc "The track's own swing resolution, one of swing-resolution-options")
         (fts       :string :set (track-setting "fts")
                    :doc "Scale name: one of project.fts-options, an imported scale's name, * once degrees are edited")
         (tuning    tuning  :doc "The scale editor's state (set! its root, morph, mode and degrees)")
         (accumulator :string :set (track-setting "accumulator") :doc "One of project.accumulator-options")
         (accum-mode :string :set (track-setting "accum-mode") :doc "One of accum-mode-options")
         (accum-limit :number :range (0 127) :set (track-setting "accum-limit"))
         (output    bus     :set set-track-output
                    :doc "The bus the track's audio goes to (the main mix bus for main); nil for sends only. One of project.output-options")
         (mod-output :bool  :doc "Has a mod output (a modulator, or an instrument exposing one)")
         (mod-out-level :number :range (0 1) :doc "Mod output port level")
         (mod-in-1  :number :range (0 1) :doc "Mod input port levels, inputs 1-4 (Ext1-4); (mod-in-level t i) binds input i")
         (mod-in-2  :number :range (0 1))
         (mod-in-3  :number :range (0 1))
         (mod-in-4  :number :range (0 1))
         (bar-transposes (list-of :number)
                    :doc "Per 16-step bar of the pattern, semitones; (set-bar-transpose! t bar v)")
         (delete-target :bool :set set-track-delete-target :doc "Among the mixer's delete target (one track or several)")
         ;; The arrangement.
         (clips     (list-of clip) :doc "The clips on the track's arrangement lane, in time order")
         (cells     (list-of cell) :doc "The track's patterns (the mixer's clip grid), by pattern id")
         (governed  :int    :doc "take-none, take-governed (a take plays on the lane: steps dimmed and locked) or take-latched (a take lane the performer latched away)")
         (latched   :bool   :set set-track-latched
                    :doc "Latched away from the song by a manual launch; set false to hand the lane back to the song")
         (pad       pad     :doc "The drum rack pad this member track backs, or nil")
         (variants  (list-of variant) :doc "The track's p-lock variants, by label (stamp-variant!)")))

;; A clip on a track's arrangement lane: (nth t.clips 0). Keyed by its stable
;; clip id: moving or resizing it keeps the instance.
(def-kind clip
  :key (track cid)
  :host ((track  track  :doc "The track whose lane holds the clip")
         (cid    :int    :doc "The host's stable clip id")
         (start  :number :set (clip-setter "start") :doc "First beat; setting it moves the clip (its length stays)")
         (end    :number :set (clip-setter "end") :doc "End beat (exclusive); setting it resizes the clip")
         (cell   cell    :set set-clip-cell :doc "The pattern the clip plays, a cell of its track; nil for a take clip")
         (take   :int    :doc "The take the clip plays, from 0; -1 for a pattern clip")
         (offset :number :doc "Where the clip starts in its source, in steps")
         (num-steps :int :doc "The source's length in steps: one pattern cycle, or the whole take")
         (length :number :doc "The source's length in beats: one pattern cycle, or the whole take")
         (events (list-of (list-of :number))
                 :doc "The source's notes, each (time transpose velocity duration), time and duration in steps")
         (dot    :bool   :doc "The clip resolves a sound (it shows the sound identity dot)")
         (dot-color :rgb :doc "The sound's palette color, themed; the timeline's gray when it has none")))

;; One of a track's patterns, a cell of the mixer's clip grid: (nth t.cells 0).
;; Keyed by its stable pattern id.
(def-kind cell
  :key (track pid)
  :host ((track    track :doc "The track whose pool holds the pattern")
         (pid      :int  :doc "The pattern's stable id in its track's pool")
         (active   :bool :doc "The track plays this pattern")
         (assigned :bool :doc "The current scene's cell on the track")
         (override :bool :doc "The track plays a launched pattern instead of its scene's (true on every cell of the track)")
         (queued   :bool :doc "Waiting for a quantized launch")
         (selected :bool :set set-cell-selected :doc "Selected as the mixer's delete target")
         (banks    (list-of bank) :doc "The scene banks whose scenes use the pattern; empty when none does yet")))

;; A scene change on the song's scene lane, as the span it governs:
;; (nth song.spans 0). Scene changes have no ids, so spans are positional: an
;; edit re-pushes the values.
(def-kind scene-span
  :key (index)
  :host ((index :int    :doc "Position on the scene lane, from 0")
         (scene scene   :doc "The scene that plays over the span")
         (start :number :doc "First beat")
         (end   :number :doc "End beat (exclusive): the next scene change, or the song's end")))

;; The arrangement and song playback.
(def-kind song
  :key ()
  :host ((exists :bool :doc "The project has a committed song")
         (mode   :string :doc "stopped, song-playback or arrangement-capture")
         (recording-kind :string :doc "Empty while not recording, else take (arrangement capture) or dub (overdub)")
         (position :number :doc "The playback position in beats (the record head while capturing over an empty song); 0 while inactive")
         (cursor :number :set (song-setter "cursor") :doc "The arrangement's edit cursor (the paste target), in beats")
         (end    :number :set (song-setter "end") :doc "End beat; setting it before a clip's end or the last scene change is an error")
         (loop   :bool   :set (song-setter "loop") :doc "Playback loops")
         (manual-latch :bool :set (song-setter "manual-latch")
                       :doc "Some lane (or the scene) is latched away from the song; set false for Back to Song")
         (scene-latched :bool :doc "The scene is the performer's (scene latch)")
         (edit-error :string :doc "Why the last arrangement edit was rejected; empty after a successful one")
         (capture-failed :bool :doc "The last arrangement capture failed")
         (capture-error :string :doc "Why it failed; empty otherwise")
         (region region :doc "The selected region, or nil; (select-region! t1 t2 start end [:scene-lane true]), (clear-region!)")
         (bound-clip clip :set set-song-bound-clip
                     :doc "The selected clip (its track's sound binds to it), or nil")
         (spans (list-of scene-span) :doc "The scene lane, in time order")))

;; The arrangement's selected region (song.region while one is selected).
(def-kind region
  :key ()
  :host ((tracks (list-of track) :doc "The tracks it spans, in order")
         (start  :number :doc "First beat")
         (end    :number :doc "End beat (exclusive)")
         (scene-lane :bool :doc "Swept in the scene lane: copy, paste and delete carry the scene changes in it too")))

;; A track's scale (the scale editor): (track 0).tuning. One per track.
(def-kind tuning
  :key (track index)
  :host ((track   track   :doc "The track whose scale this is")
         (on      :bool   :doc "A scale is on (fts is not Off)")
         (scale   :string :doc "The scale's name (an imported scale's name)")
         (custom  :bool   :doc "An imported (.scl) scale")
         (edited  :bool   :doc "Some degree is detuned or switched off")
         (root    :string :set (tuning-setter "root") :doc "The pitch class degree 0 sits on, one of tuning-root-options")
         (morph   :number :range (0 1) :set (tuning-setter "morph")
                  :doc "0 rounds every degree to its nearest semitone, 1 plays it exact")
         (mode    :string :set (tuning-setter "mode") :doc "How keys map to degrees, one of tuning-mode-options")
         (period  :number :doc "Cents per period (1200 for an octave)")
         (degrees (list-of degree) :doc "The scale's degrees, in order; empty while off")))

;; One degree of a scale: (nth tn.degrees 2).
(def-kind degree
  :key (tuning index)
  :host ((tuning  tuning  :doc "The scale the degree belongs to")
         (index   :int    :doc "Position in the scale, from 0")
         (base    :number :doc "The scale's own pitch of the degree, cents above the root")
         (offset  :number :range (-1200 1200) :set (degree-setter "offset") :doc "Cents added to base")
         (enabled :bool   :set (degree-setter "enabled") :doc "In the scale (false skips the degree)")
         (pitch   :number :doc "The sounding pitch, cents above the root (offset and morph applied)")
         (label   :string :doc "The sounding pitch's note name and cents")
         (ratio   :string :doc "The nearest simple just ratio (empty when none, or the period is not an octave)")))

;; A mixer bus, the main mix included.
(def-kind bus
  :key (index)
  :host ((index  :int    :doc "Position in the bus list, from 0")
         (bid    :int    :doc "The host's stable bus id")
         (name   :string)
         (volume :number :range (0 1) :set set-bus-volume)
         (muted  :bool   :set set-bus-muted)
         (soloed :bool   :set set-bus-soloed)
         (peak   :number :range (0 1) :doc "Output meter level")
         (output bus     :set set-bus-output :doc "The bus this one feeds (the main mix by default); nil for the main mix")
         (output-options (list-of bus) :doc "The buses output may be set to (empty when fixed)")
         (devices (list-of device) :doc "The bus's effects, in chain order")
         (mod-in-1 :number :range (0 1) :doc "Mod input port levels, inputs 1-4 (Ext1-4); (mod-in-level b i) binds input i")
         (mod-in-2 :number :range (0 1))
         (mod-in-3 :number :range (0 1))
         (mod-in-4 :number :range (0 1))))

;; A modulation route: a track's mod output into a track's or a bus's mod
;; input. Keyed by its endpoints (track and bus ids), so a reorder keeps it.
(def-kind route
  :key (index)
  :host ((index    :int   :doc "Position in the route list, from 0")
         (source   track  :doc "The modulating track")
         (dest     track  :doc "The modulated track, or nil for a bus")
         (dest-bus bus    :doc "The modulated bus, or nil for a track")
         (input    :int   :doc "The destination's mod input, 1-4 (Ext1-4), as in mod-in-1")
         (selected :bool  :set set-route-selected :doc "Selected as the mixer's delete target")))

;; A track group (or drum rack) and the bus it routes to.
(def-kind group
  :key (index)
  :host ((index     :int    :doc "Position in the group list, from 0")
         (gid       :int    :doc "The host's stable group id (legacy SEQ.groups :id)")
         (name      :string)
         (color     :rgb)
         (collapsed :bool   :set set-group-collapsed)
         (rack      :bool   :doc "A drum rack")
         (tracks    (list-of track) :doc "Member tracks, in order")
         (bus       bus     :doc "The group's bus, or nil")
         (racks     (list-of group) :doc "The drum racks this (plain) group draws inside its block")
         (parent    group   :doc "The group drawing this rack inside its block, or nil")
         ;; Drum racks (empty, nil or false on a plain group).
         (armed     :bool   :set set-group-armed
                    :doc "The live keyboard plays this rack's pads (one rack at a time; arming disarms its member tracks)")
         (pads      (list-of pad) :doc "The rack's pads, in pad order")
         (clips     (list-of rack-clip) :doc "The rack's clip bank, in order")
         (rack-clip rack-clip :doc "The clip the current scene plays, or nil (silent, or no clips)")
         (legacy    :bool   :doc "A rack without a clip bank: its members play the project scenes (convert-rack-to-clips!)")
         (groove    groove  :doc "The rack's own groove (what its clips follow unless they own one)")))

;; A drum rack pad: (nth g.pads 0). Keyed by its member track, so moving the
;; pad to another note (a swap included) or reordering keeps the instance.
(def-kind pad
  :key (group tid)
  :host ((group     group  :doc "The rack")
         (track     track  :doc "The member track the pad plays (its steps: p.track.steps)")
         (note      :int    :range (-36 51) :set (pad-setter "note")
                    :doc "The note the pad answers to (0 is C4); setting an occupied note swaps the two pads")
         (label     :string :doc "The note's name (C1 … D#8)")
         (choke     :int    :range (0 16) :set (pad-setter "choke") :doc "Choke group, 0 for none")
         (role      :string :set (pad-setter "role")
                    :doc "The explicit drum role, one of pad-role-options; empty for Standard")
         (role-tag  :string :doc "The effective role's short tag (BD, SD, …); empty when none")
         (role-label :string :doc "The effective role's name; empty when none")
         (standard-role :string :doc "The role key the standard layout infers from the note (as role, one of pad-role-options); empty when none")
         (standard-role-label :string :doc "That role's name; empty when none")
         (triggered :bool   :doc "Sounding: lit for a moment after each hit, however it was played")))

;; A clip of a drum rack's own scene axis: (nth g.clips 0). Keyed by its
;; stable clip id. Launch with (launch-rack-clip! rc).
(def-kind rack-clip
  :key (group cid)
  :host ((group  group  :doc "The rack")
         (cid    :int    :doc "The host's stable clip id")
         (index  :int    :doc "Position in the rack's bank, from 0")
         (name   :string :set (rack-clip-setter "name") :doc "Non-empty")
         (active :bool   :doc "The current scene plays it")
         (scenes (list-of scene) :doc "The scenes that play it")
         (groove groove  :doc "The clip's own groove, or nil while it follows the rack's")
         (own-groove :bool :set (rack-clip-setter "own-groove")
                     :doc "Plays its own groove (set true: a copy of the rack's; false: follow the rack's again)")))

;; A rack's groove setting: g.groove (the rack's own) or rc.groove (a clip's
;; own). What a groove does to the rack's hits: the pool groove it plays and
;; how much of it.
(def-kind groove
  :key (group clip)
  :host ((group    group  :doc "The rack")
         (clip     rack-clip :doc "The clip owning it, or nil for the rack's own")
         (pool-groove pool-groove :set set-groove-pool-groove :doc "The pool groove played, or nil for none")
         (enabled  :bool   :set (groove-setter "enabled") :doc "False bypasses it, keeping the rest")
         (timing   :number :range (0 1.5) :set (groove-setter "timing"))
         (velocity :number :range (0 1.5) :set (groove-setter "velocity"))
         (random   :number :range (0 1) :set (groove-setter "random"))
         (scale    :number :set (groove-setter "scale") :doc "Time scale, one of groove-scale-options")
         (grid     :string :doc "The grid it plays at (1 bar · 1/16, …), scale applied; empty with none")
         (slots    :int    :doc "Slots in one bar of the lanes (16 with no groove)")
         (cells    (list-of :number) :doc "The All lane: each slot's offset from the grid, in slots (late +, early -)")
         (measured (list-of :bool) :doc "Per slot of the All lane: measured rather than filled")
         (pads     (list-of pad-groove) :doc "Each pad's share and lane, in note order")))

;; One pad's share of a groove: (nth gr.pads 0).
(def-kind pad-groove
  :key (groove tid)
  :host ((groove   groove :doc "The groove")
         (pad      pad    :doc "The pad")
         (amount   :number :range (0 1) :set (pad-groove-setter "pad-amount") :doc "Scales the groove on this pad")
         (enabled  :bool   :set (pad-groove-setter "pad-enabled") :doc "False leaves the pad straight (its amount is kept)")
         (cells    (list-of :number) :doc "The pad's lane: each slot's offset from the grid, in slots")
         (measured (list-of :bool) :doc "Per slot: measured rather than filled")))

;; A groove of the project's pool: (nth project.groove-pool 0). Positional;
;; the instance follows its groove id across reorders, like a bus.
(def-kind pool-groove
  :key (index)
  :host ((index     :int    :doc "Position in the pool, from 0")
         (groove-id :int    :doc "The host's stable groove id")
         (name      :string :set set-pool-groove-name :doc "Non-empty")
         (grid      :string :doc "Its own grid: 1 bar · 1/16, …")
         (racks     (list-of group) :doc "The racks playing it (their own groove or a clip's)")))

;; A groove file of the library (factory or user): (nth project.groove-library
;; 0). (use-library-groove! gr lg) copies it into the pool and plays it.
(def-kind library-groove
  :key (index)
  :host ((index :int    :doc "Position in the library listing, from 0")
         (choice :string :doc "Its picker key: factory:<stem> or user:<stem>")
         (name  :string)
         (tier  :string :doc "factory or user")))

;; A scene; `bank` holds it. Launch with (launch! s).
(def-kind scene
  :key (index)
  :host ((index  :int    :doc "Position in the scene list, from 0")
         (number :int    :doc "1-based position within its bank")
         (name   :string)
         (active :bool   :doc "The playing scene")
         (queued :bool   :doc "Waiting for a quantized launch")
         (bank   bank)))

(def-kind bank
  :key (index)
  :host ((index   :int    :doc "Position in the bank list, from 0")
         (label   :string :doc "A, B, … with the bank name after a dash when it has one")
         (scenes  (list-of scene))
         (playing :bool   :doc "Holds the playing scene")))

(def-kind transport
  :key ()
  :host ((playing         :bool :set set-transport-playing)
         (recording       :bool :set set-transport-recording)
         (scene           scene :doc "The playing scene")
         (queued          scene :doc "The scene a quantized launch waits for, or nil")
         (launch-quantize :string :doc "Scene launch quantization: off, 1 bar, …")
         (bpm             :int    :range (20 300) :set set-transport-bpm)
         (position        :int    :doc "Transport step counter")
         (metronome       :bool   :set set-transport-metronome)
         (roll-mode       :bool   :set set-transport-roll-mode)
         (record-quantize :string :set set-transport-record-quantize
                          :doc "Live-record quantization: off, 1/16, …, 1 bar")
         (roll-rate       :string :set set-transport-roll-rate :doc "One of roll-rate-options")
         (sequence-rolling :bool  :doc "A sequence roll is held")))

;; The master output.
(def-kind master
  :key ()
  :host ((peak-l    :number :range (0 1))
         (peak-r    :number :range (0 1))
         (recording :bool   :set set-master-recording :doc "Recording the master output to a file")))

;; The audio engine.
(def-kind engine
  :key ()
  :host ((cpu-load   :number :doc "Audio callback load, percent")
         (latency-ms :number :doc "Plugin-delay-compensation latency")
         (overloaded :bool   :doc "An audio deadline was missed in the last two seconds")
         (compiling  :bool   :doc "An effect compile is running")))

(def-kind selection
  :key ()
  :host ((track  track :set select-track :doc "The current track")
         (tracks (list-of track) :doc "The multi-track selection")
         (steps  (list-of step) :doc "The current track's selected steps, in order")
         (cursor-step step :set set-cursor-step
                      :doc "The step under the step cursor (current track); setting it selects the step's track")
         (edit-step step :doc "The step the step panel edits: the first selected step, else cursor-step")
         (rack-slot :int :doc "The current drum rack's selected slot, -1 when the current track is no rack")
         (auto-follow :bool :doc "The view follows the playhead (paused for a while after an edit)")))

;; The collections, for (tracks), (scenes), (banks), (buses), (groups) and
;; (routes), and the option lists the host owns.
(def-kind project
  :key ()
  :host ((tracks (list-of track))
         (scenes (list-of scene))
         (banks  (list-of bank))
         (buses  (list-of bus))
         (groups (list-of group))
         (routes (list-of route))
         (fts-options (list-of :string) :doc "The built-in scales, for track.fts")
         (sync-options (list-of :string) :doc "step.sync labels, by value")
         (accumulator-options (list-of :string) :doc "Built-in and script accumulators, for track.accumulator")
         (output-options (list-of bus) :doc "The buses a track's output may be set to (nil is sends only)")
         (groove-pool (list-of pool-groove) :doc "The project's grooves, in pool order")
         (groove-library (list-of library-groove) :doc "The groove files of the library, factory first")
         (macros (list-of macro) :doc "The project's macros, in macro order")))

;; ── Collections and actions ──

(def tracks () project.tracks)
(def scenes () project.scenes)
(def banks () project.banks)
(def buses () project.buses)
(def groups () project.groups)
(def routes () project.routes)
(def macros () project.macros)

;; Launch a scene with the transport's launch quantization.
(def launch! (s)
  (host-command "switch-pattern"
    (dict :idx s.index :quantize transport.launch-quantize)))

;; Copy scene s to a new scene at the end of its bank, which then plays.
(def clone-scene! (s) (host-command "clone-pattern" (dict :idx s.index)))

;; Delete scene s (never the last one); the playing scene keeps playing
;; unless it is s.
(def delete-scene! (s) (host-command "delete-pattern" (dict :idx s.index)))

;; track.governed values.
(def take-none 0)
(def take-governed 1)
(def take-latched 2)

;; Launch cell c on its track with the transport's launch quantization: the
;; current scene's cell becomes c's pattern. The track (by id) and the
;; current scene resolve when the command lands.
(def launch-cell! (c)
  (host-command "set-scene-cell"
    (dict :track-id c.track.tid :pattern-id c.pid :quantize transport.launch-quantize)))

;; Select the region from track t1 to track t2 (either order) between beats
;; start and end (song.region); a degenerate one clears it. :scene-lane true
;; also sweeps the scene lane, so copy, paste and delete carry its scene
;; changes.
(def select-region! (t1 t2 start end &key (scene-lane false))
  (host-command "set-song-region"
    (dict :track-ids (list t1.tid t2.tid) :start start :end end :scene-lane scene-lane)))
(def clear-region! () (host-command "set-song-region" nil))

;; step.lock-kind values.
(def lock-none 0)
(def lock-seq 1)
(def lock-variant 2)

;; Device d's param named name, or nil.
(def device-param (d name)
  (first (filter (lambda (p) (= p.name name)) d.params)))

;; P-lock param p to v (display units) on steps, a list of step instances of
;; p's track (any other track's step is an error); one undo entry. Steps
;; already locked to v are left alone. A bus effect's params take no p-locks.
(def lock-param! (p steps v)
  (host-command "set-device-param-locks"
    (merge (device-target p.device)
           :param-idx p.index :steps (map (lambda (s) s.index) steps)
           :step-tracks (map (lambda (s) s.track.tid) steps) :value v)))

;; Clear param p's p-locks on steps (a list of step instances of p's track);
;; one undo entry. (A drum rack slot instrument's locks cannot be cleared
;; yet: an error.)
(def unlock-param! (p steps)
  (host-command "clear-device-param-locks"
    (merge (device-target p.device)
           :param-idx p.index :steps (map (lambda (s) s.index) steps)
           :step-tracks (map (lambda (s) s.track.tid) steps))))

;; Set cell cell (row * tz.cols + col) of tensor tz to v (the device's own
;; cells, never a p-lock; v within tz.min-tz.max). A drag's set!s join one
;; undo entry.
(def set-tensor-cell! (tz cell v)
  (host-command "set-device-tensor"
    (merge (device-target tz.device) :tensor-idx tz.index :cell cell :value v)))

;; Stamp variant v (one of t.variants) onto steps (step instances of track
;; t); nil v clears the steps' variant locks. One undo entry.
(def stamp-variant! (t steps v)
  (host-command "stamp-variant"
    (dict :track-id t.tid :label (if v v.label "def")
          :steps (map (lambda (s) s.index) steps)
          :step-tracks (map (lambda (s) s.track.tid) steps))))

;; Stamp key-lock variant v (one of d.variants) onto keys notes (MIDI note
;; numbers) of instrument d; nil v clears those keys' variant locks.
(def stamp-key-variant! (d notes v)
  (host-command "stamp-key-variant"
    (merge (device-target d) :label (if v v.label "def") :notes notes)))

;; Load track t's next (dir 1) or previous (dir -1) preset, wrapping.
(def step-preset! (t dir)
  (host-command "step-instrument-preset" (dict :track t.index :delta dir)))

;; Scale editor actions on tn, a track's tuning; one undo entry each.
;; Drop every degree's offset and switch every degree back on.
(def reset-tuning! (tn) (tuning-edit tn "reset" nil))
;; Snap every degree to its nearest simple just ratio.
(def justify-tuning! (tn) (tuning-edit tn "just" nil))
;; Detune every degree at random, by up to cents (0-600).
(def randomize-tuning! (tn cents) (tuning-edit tn "rand" cents))
;; Stretch the scale by cents per period (-600-600).
(def stretch-tuning! (tn cents) (tuning-edit tn "stretch" cents))

;; Transpose bar (16-step page) bar of track t's pattern by v semitones
;; (-60-60; out of range is an error).
(def set-bar-transpose! (t bar v)
  (host-command "set-track-bar-transpose" (dict :track-id t.tid :bar bar :value v)))

;; A binding to mod input i (1-4, as in mod-in-1) of x, a track or a bus;
;; any other i is an error (reported; the value is false).
(def mod-in-level (x i)
  (if (= i 1) #'x.mod-in-1
    (if (= i 2) #'x.mod-in-2
      (if (= i 3) #'x.mod-in-3
        (if (= i 4) #'x.mod-in-4
          (seq-error (str "mod-in-level: no input " i " (inputs are 1-4)")))))))

;; ── Drum racks ──

;; Hit pad p as a pad key does (its member track at base pitch, so choke and
;; the member's effects apply).
(def trigger-pad! (p)
  (host-command "trigger-rack-pad" (dict :group-id p.group.gid :track-id p.track.tid)))

;; Launch rack clip rc in the current scene, with the transport's launch
;; quantization (one undo entry, as a clip launch).
(def launch-rack-clip! (rc)
  (host-command "launch-rack-clip"
    (dict :group-id rc.group.gid :clip-id rc.cid :quantize transport.launch-quantize)))

;; Silence rack g in the current scene.
(def silence-rack! (g)
  (host-command "launch-rack-clip"
    (dict :group-id g.gid :clip-id 0 :quantize transport.launch-quantize)))

;; Save what rack g plays as a new clip named name.
(def save-rack-clip-as! (g name)
  (host-command "save-rack-clip-as" (dict :group-id g.gid :name name)))

(def delete-rack-clip! (rc)
  (host-command "delete-rack-clip" (dict :group-id rc.group.gid :clip-id rc.cid)))

;; Give a legacy rack (g.legacy) its clip bank: a clip per scene.
(def convert-rack-to-clips! (g)
  (host-command "convert-rack-to-clips" (dict :group-id g.gid)))

;; Copy library groove lg into the pool (reusing a pool groove of the same
;; feel) and make it groove gr's.
(def use-library-groove! (gr lg)
  (host-command "set-rack-groove"
    (dict :group-id gr.group.gid :clip-id (groove-clip-id gr) :key lg.choice)))

;; Make groove gr the rack's own, and every clip follow it.
(def apply-groove-to-all-clips! (gr)
  (host-command "apply-rack-groove-to-all-clips"
    (dict :group-id gr.group.gid :clip-id (groove-clip-id gr))))

;; Extract a groove from rack g's clip into the pool: bars 1 or 2,
;; resolution "1/16" or "1/32"; with quantize the source is straightened and
;; the groove played, in one undo entry.
(def extract-groove! (g name bars resolution quantize)
  (host-command "extract-rack-groove"
    (dict :group-id g.gid :name name :bars bars :resolution resolution :quantize quantize)))

;; Pool groove edits: duplicating or deleting is one undo entry each
;; (deleting turns the groove off on every rack playing it, in that entry).
;; Saving to the library writes a file, which undo does not take back.
(def duplicate-groove! (pg) (host-command "duplicate-pool-groove" (dict :groove-id pg.groove-id)))
(def delete-groove! (pg) (host-command "delete-rack-groove" (dict :groove-id pg.groove-id)))
(def save-groove-to-library! (pg)
  (host-command "save-groove-to-library" (dict :groove-id pg.groove-id)))
