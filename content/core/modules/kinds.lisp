;; eseq.kinds — the host kinds: what a view can be built from
;; (docs/kind-bindings-spec.md §3.4, §4, §9, §14).
;;
;; Each kind here is a projection of the sequencer's own state. The host
;; registers the instances (tracks, scenes, banks, buses, groups, devices
;; (a track's chain, MIDI effects and drum rack slots, a bus's effects),
;; sends, clips, cells, scene spans, drum rack pads, rack clips and grooves,
;; tensors, fixed modulators, p-lock variants, project and drum rack macros
;; and their mappings, process classes, the browser's preset files and rack
;; slots, the open sound palette's sounds, the editor's macros and assets,
;; Patch Learn's rows, a MIDI capture's lanes and notes, MIDI inputs, graph
;; sequencers and their nodes;
;; steps on first read of `t.steps`, params (with their
;; modulation lanes) on first read of `d.params`, a track's processes (and
;; their lanes, inlets, ports and state cells) on first read of `t.processes`
;; or `t.lanes`, the piano roll's notes on first read of `piano-roll.notes`, a
;; graph node's edges and params on first read of `n.edges` / `n.params`, an
;; edge's params on first read of `e.params`),
;; pushes their `:host` fields,
;; and checks at startup that it publishes exactly the fields declared below
;; (crates/sequencer/src/ui/host_kinds/). A view imports what it uses:
;;
;;   (import eseq.kinds :refer (track tracks transport scenes banks selection launch!))
;;
;; Read a field by value (`t.name`, re-renders the reader) or bind it
;; (`#'t.volume`, repaints only). Writable fields carry `:set`: `(set! t.volume
;; 0.5)`, `(toggle! t.muted)`, `(toggle! s.active)`, `(set! selection.track t)`,
;; `(set! t.swing 56)`, `(set! t.output nil)`, `(set! tn.morph 0.5)` (tn = t.tuning),
;; `(set! n.start 4)` (n a note of piano-roll.notes).
;; The host computes a field only while something observes it (a reader or a
;; held `#'`); reading an unobserved one asks the host for its value.

(module eseq.kinds)

(export track scene bank bus group transport selection project master engine
        song region
        tracks scenes banks buses groups routes macros
        launch! clone-scene! delete-scene! step-preset!
        device-param lock-param! unlock-param! lock-strip! unlock-strip!
        set-tensor-cell! stamp-variant! stamp-key-variant!
        lock-none lock-seq lock-variant
        reset-tuning! justify-tuning! randomize-tuning! stretch-tuning!
        set-bar-transpose! mod-in-level
        mute-group-options accum-mode-options tuning-root-options tuning-mode-options
        voice-priority-options mono-trigger-options swing-resolution-options
        roll-rate-options launch-quantize-options record-quantize-options
        launch-cell! select-region! clear-region! take-none take-governed take-latched
        pad-role-options groove-scale-options
        trigger-pad! launch-rack-clip! silence-rack! save-rack-clip-as! delete-rack-clip!
        convert-rack-to-clips! use-library-groove! apply-groove-to-all-clips! extract-groove!
        duplicate-groove! delete-groove! save-groove-to-library!
        process-library set-process-enabled! set-inlet! set-lane-steps! move-process!
        add-process! remove-process! bind-port! add-fanout! unbind-port! clear-port!
        remove-fanout!
        browser sound-palette editor learn retro song-export settings agent
        apply-sound! apply-sound-with-mix! fork-sound! open-sound-palette! close-sound-palette!
        learn-method-options learn-refine-mode-options
        piano-roll add-note! delete-notes! pitch-min pitch-max
        graph graphs graph-of graph-param-named graph-edge-to set-group-gain!
        set-group-coupling! gate-generator! graph-timebase-options graph-quantize-options
        graph-max-poly-selection-options)

;; Short fixed option lists (the host checks they match its own). The lists
;; the host owns (scales, step sync resolutions, accumulators, track outputs,
;; the step params a process port writes) are `project` fields:
;; project.fts-options, project.sync-options, project.step-param-options, ….
(def mute-group-options '("Off" "1" "2" "3" "4" "5" "6" "7" "8"))
(def accum-mode-options '("rtz" "clip" "rvtz" "rvbp"))
(def tuning-root-options '("C" "C#" "D" "D#" "E" "F" "F#" "G" "G#" "A" "A#" "B"))
(def tuning-mode-options '("Snap" "Map"))
(def voice-priority-options '("Last" "High" "Low"))
(def mono-trigger-options '("retrig" "legato"))
(def swing-resolution-options '("1/16" "1/8" "1/4" "1/2"))
(def roll-rate-options '("4" "4T" "8" "8T" "16" "16T" "32" "32T"))
;; transport.launch-quantize's and transport.record-quantize's choices.
(def launch-quantize-options '("off" "1/16" "1/8" "1/4" "1/2" "1 bar"))
(def record-quantize-options '("off" "1/16" "1/8" "1/4" "1/2" "1 bar"))
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
(def set-transport-launch-quantize (tr v) (host-command "set-scene-launch-quantize" v))
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
;; The delete-target setters (a group's, a route's, a cell's) touch only UI
;; thread state, so they apply at once: true makes `target` (a `kind` delete
;; target, as `seq-set-delete-target` takes it) the mixer's delete target;
;; false clears the delete target when it is that one.
(def set-delete-target (kind target v)
  (if v
    (seq-set-delete-target kind target)
    (if (seq-delete-target? kind target) (seq-clear-delete-target) nil)))
;; g, by group id.
(def set-group-delete-target (g v)
  (set-delete-target :mixer-group (dict :group-id g.gid) v))
(def route-target (r)
  (dict :source r.source.index :dest-kind (if r.dest "track" "bus")
        :dest (if r.dest r.dest.index r.dest-bus.bid) :input (- r.input 1)))
(def set-route-selected (r v) (set-delete-target :mod-route (route-target r) v))
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
;; A cell (track position, pattern id) as the mixer's delete target.
(def set-cell-selected (c v)
  (set-delete-target :track-pattern (dict :track c.track.index :pattern-id c.pid) v))
;; A piano roll note by its track's id and its note id (spec §14, stage 7e):
;; pitch, start, length and velocity move or reshape it (one undo entry each;
;; a drag's set!s join one), selected selects it in the piano roll.
;; Values (spec §14.2c): pitch an integer in pitch-min to pitch-max, start a
;; step from 0 to below piano-roll.focus-num-steps, length 1/32 to 32 steps,
;; velocity 0 to 1.
(def note-setter (field)
  (lambda (n v)
    (host-command "set-note" (dict :track-id n.track.tid :nid n.nid :field field :value v))))
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
;; A device's own fields (set-device): a rack slot's voices and strip
;; controls, the delete target, an instrument's base note.
(def device-setter (field)
  (lambda (d v)
    (host-command "set-device" (merge (device-target d) :field field :value v))))

;; Macros (spec §14.2g): a project macro by its id, a drum rack's macro by its
;; rack's device and its index, a mapping by its macro and its position.
(def macro-setter (field)
  (lambda (m v) (host-command "set-macro" (dict :macro-id m.mid :field field :value v))))
;; A scene macro's scene by its position, its tracks by their stable ids.
(def set-macro-target-scene (m s)
  (host-command "set-macro"
    (dict :macro-id m.mid :field "target-scene" :value (if s s.index nil))))
(def set-macro-tracks (m ts)
  (host-command "set-macro"
    (dict :macro-id m.mid :field "tracks" :value (map (lambda (t) t.tid) ts))))
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

;; Process lanes (spec §14.2h). A process (a slot of a track's composed
;; chain) is addressed by its track's stable id and its proc-id, its inlets,
;; ports and fan-out entries by their process and their name or position,
;; all resolved when the command lands. Field setters edit this track only
;; (a project lane forks for it); the actions take :all true to edit the
;; shared project slot (every track). One undo entry each.
(def process-target (p) (dict :track-id p.track.tid :proc-id p.proc-id))
(def port-address (pt) (merge (process-target pt.process) :port pt.name))
(def fanout-address (fo) (merge (port-address fo.port) :index fo.index))
(def edit-process (address op &rest more)
  (host-command "edit-process" (apply merge address :op op more)))
(def fanout-setter (op)
  (lambda (fo v) (edit-process (fanout-address fo) op :value v)))

;; Run (or bypass) p; :all true sets the shared project lane for every track.
(def set-process-enabled! (p v &key (all false))
  (edit-process (process-target p) "enabled" :value v :all all))

;; Set inlet i to v; :all true sets the shared project lane's.
(def set-inlet! (i v &key (all false))
  (edit-process (process-target i.process) "inlet" :inlet i.name :value v :all all))

;; The browser, the sound palette, the editor and the app's views (spec
;; §14.2i). A sound by its track's stable id and its patch id, a MIDI input by
;; its device id, resolved when the command lands.
(def set-sound-name (s v)
  (host-command "sound-rename" (dict :track-id s.track.tid :kind "patch" :entity s.patch-id :name v)))
(def set-editor-run-mode (e v)
  (host-command "set-draft-instrument-run-mode" (dict :run-mode v)))
;; Patch Learn's training settings: one of the method or refine mode options,
;; an integer in the field's range (cma-population 0 or at least 4), a sigma
;; above 0 up to 10, a positive pitch or gate length; anything else is an error.
(def learn-setter (field)
  (lambda (l v) (host-command "set-learn" (dict :field field :value v))))
(def set-audio-workers-choice (st v) (host-command "audio-set-workers" (dict :choice v :strict true)))
(def set-midi-device-enabled (d v)
  (host-command "midi-set-enabled" (dict :id d.device-id :enabled v)))

;; Graph sequencers (spec §14.2k): a graph by its sequencer id (gid; a
;; created instance's id), a node by its index, an edge by its endpoints, a
;; param by its node or edge and its name, all resolved when the command
;; lands (a graph that is gone, or a node past the active count, is an
;; error). Edits go into the current scene's overrides, one undo entry per
;; field (undo restores that field alone); a drag's set!s on one field join
;; one entry. Values (spec §14.2c): labels among their options
;; (case-insensitive), numbers finite and in range, integers whole; anything
;; else is an error.
(def graph-edit (g field v &rest more)
  (host-command "set-graph" (apply merge (dict :graph-id g.gid :field field :value v) more)))
(def graph-setter (field) (lambda (g v) (graph-edit g field v)))
(def graph-node-setter (field)
  (lambda (n v) (graph-edit n.graph field v :node n.index)))
;; A node's route: a track of the graph's owner (a rack-owned graph's
;; member), or nil for off.
(def set-graph-node-route (n t)
  (graph-edit n.graph "route" nil :node n.index :track-id (if t t.tid nil)))
;; The tracks a node listens to (empty: off); setting them stops following
;; the route.
(def set-graph-node-seeds (n ts)
  (graph-edit n.graph "seeds" (map (lambda (t) t.tid) ts) :node n.index))
(def set-graph-param-value (p v)
  (if p.node
    (graph-edit p.node.graph "param" v :node p.node.index :param p.name)
    (graph-edit p.edge.from.graph "edge-param" v
      :node p.edge.from.index :to p.edge.to.index :param p.name)))

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
         (variant-color :rgb  :doc "The step's p-lock variant color (gray for sequencer-only locks)")
         (variant variant :doc "The step variant the step plays (one of its track's variants); nil when it plays none")))

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
         (has-locks :bool :doc "Some step of the track's pattern locks this send")
         (process-mapped :bool :doc "An enabled process slot of the track writes this send")
         (process-value :number :doc "The level a process last wrote here; display when none has")))

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
         ;; The descriptor's UI metadata (spec §14.2l): empty when it has none.
         (group   :string :doc "The section a param grid groups it under")
         (env     :string :doc "The envelope it belongs to (an ADSR editor's)")
         (role    :string :doc "Its role in that envelope (attack, decay, …)")
         (display-name :string :doc "The name the instrument's source spells it (name is the host id)")
         (asset-options :any :doc "An options reference that did not resolve: (dict :tensor :file :key …); nil otherwise (a resolved one is options)")
         ;; Modulation and process display.
         (mod-targets (list-of mod-target) :doc "The modulation lanes onto this param")
         (mod-offset :number :doc "How far modulation moves value now (display units); 0 while unmodulated or not sampled")
         (mod-value  :number :doc "Where modulation moves value now; value while unmodulated")
         (mod-scale  :number :doc "An exponential destination's modulation ratio (mod-value / value); 1 otherwise")
         (process-mapped :bool :doc "An enabled process slot of the track writes this instrument param")
         (process-value :number :doc "The value a process last wrote here (display units); value when none has")
         (process-clamped :bool :doc "That write hit the end of the param's range")
         (key-locks (list-of (list-of :number)) :doc "An instrument param's key locks, (note value) per locked key, ascending (display units)")
         (step-locks (list-of (list-of :number)) :doc "The p-locks of the track's pattern (its first num-steps steps), (step value) per locked step, ascending (display units)")))

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
         ;; A scene macro's config (spec §14.2l): each set! is one undo
         ;; entry; a mapped macro reads nil / false / empty and takes none.
         (target-scene scene :set set-macro-target-scene
                       :doc "A scene macro's scene; nil for a mapped one")
         (morph-params :bool :set (macro-setter "morph-params")
                       :doc "A scene macro morphs the params that differ in its scene")
         (steal-patterns :bool :set (macro-setter "steal-patterns")
                         :doc "A scene macro launches its scene's patterns")
         (quantize :string :set (macro-setter "quantize")
                   :doc "A scene macro's steal quantization (off, sixteenth, bar); empty for a mapped one")
         (tracks (list-of track) :set set-macro-tracks
                 :doc "The tracks a scene macro acts on (every track while it names none); empty for a mapped one")
         (diff-count :int :doc "How many params differ between now and a scene macro's scene (what it morphs); 0 for a mapped one. Computed while observed (an unobserved scene macro's reads its last value, nil before)")))

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
         (step-locks (list-of (list-of :number)) :doc "The p-locks of the track's pattern, (step value) per locked step, ascending")
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
         (enabled :bool   :set (device-setter "enabled")
                  :doc "False while bypassed (a rack slot: switched off). Only a rack slot's is settable (one undo entry); any other device's set! is an error")
         (params  (list-of param) :doc "The device's parameters, in descriptor order")
         (playhead :number :doc "A sampler's playing position in seconds, 0 when idle or not a sampler")
         (devices (list-of device) :doc "The devices it holds: a drum rack's slots, a rack slot's effects; empty otherwise")
         (container device :doc "The device whose devices holds this one (a rack slot's rack, a rack slot effect's slot), or nil")
         (voices  :int :set (device-setter "voices")
                  :doc "A drum rack slot's own voices, 1 (mono) to 16 (the setter checks; no declared range: 0 reads for any other device, which takes no set!); a drag's set!s join one undo entry. lock-strip! p-locks it; voices-display is the voices shown")
         (delete-target :bool :set (device-setter "delete-target")
                  :doc "The delete target (Backspace deletes it). Track effects are targets only on the current track; an instrument never")
         ;; Panel extras (spec §14.2g).
         (base-note :number :range (-48 48) :set (device-setter "base-note")
                    :doc "A track instrument's base note offset, or a drum rack slot's own (a strip control: lock-strip! p-locks it), in semitones; 0 for any other device (which takes no set!). A drag's set!s join one undo entry")
         (mod-phases (list-of :number) :doc "Each modulation source's (1-4) cycle position; -1 when it has none or nothing samples it")
         (tensors (list-of tensor) :doc "The device's tensors (tables of cells)")
         (key-locked-notes (list-of :int) :doc "A track instrument's keys holding a key lock, ascending")
         (variants (list-of variant) :doc "A track instrument's key-lock variants (stamp-key-variant!)")
         (macros (list-of rack-macro) :doc "A drum rack's macros (on its instrument device); empty otherwise")
         ;; A drum rack slot's strip controls (spec §14.2f): the slot's own
         ;; value (setting it never p-locks; lock-strip! does), the value
         ;; shown (the p-lock at the current track's selected or playing step,
         ;; else a rack macro mapped onto it, else the own value) and whether
         ;; a p-lock supplies it. Any other device reads 0 / false and takes
         ;; no set!.
         (gain :number :range (0 2) :set (device-setter "gain")
               :doc "A rack slot's own gain (linear, 1 = unity); a drag's set!s join one undo entry")
         (gain-display :number :range (0 2) :doc "The gain shown")
         (gain-locked :bool :doc "gain-display comes from a p-lock")
         (pan :number :range (-1 1) :set (device-setter "pan")
              :doc "A rack slot's own pan, -1 (left) to 1 (right); a drag's set!s join one undo entry")
         (pan-display :number :range (-1 1) :doc "The pan shown")
         (pan-locked :bool :doc "pan-display comes from a p-lock")
         (muted :bool :set (device-setter "muted") :doc "A rack slot's own mute")
         (muted-display :bool :doc "The mute shown")
         (muted-locked :bool :doc "muted-display comes from a p-lock")
         (soloed :bool :set (device-setter "soloed") :doc "A rack slot's own solo")
         (soloed-display :bool :doc "The solo shown")
         (soloed-locked :bool :doc "soloed-display comes from a p-lock")
         (choke :int :range (0 16) :set (device-setter "choke")
                :doc "A rack slot's choke group, 0 for none (no p-locks)")
         (base-note-display :number :range (-48 48)
                            :doc "The base note shown (a track instrument's: its base-note)")
         (base-note-locked :bool :doc "base-note-display comes from a p-lock")
         (voices-display :int :doc "The voices shown (the rack panel's V picker); 0 for any other device")
         ;; The panel header and its meters (spec §14.2l).
         (display-name :string :doc "The name the panel header shows: an instrument's or rack slot's without its folder or pin (a drum rack's track name, Sampler for a sampler), else name")
         (sound-binding :string :doc "A track instrument's bound sound (the header badge): the patch name, else the binding's (Take 2 · bars 0-2, Pattern 2); empty when unbound or for any other device. Computed while observed (an unobserved one reads its last value, empty before)")
         (meter :any :doc "The device's output meter selector, a device-meter's :source (names the device, not a node); nil for a MIDI effect")
         (modulators (list-of modulator) :doc "An instrument's fixed modulation sources (its descriptor's); empty otherwise")
         (modulator-phase :number :doc "A modulator instrument's envelope phase; 0 for any other device")
         (modulator-level :number :doc "A modulator instrument's output level; 0 for any other device")
         ;; Effect tables (a Filter Table's, a Convolution Reverb's).
         (table-name :string :doc "A Filter Table's loaded table (No table when none); empty for any other device")
         (table-options (list-of :string) :doc "A Filter Table's loadable tables (the table asset stems); empty for any other device")
         (table-mode :string :doc "A Filter Table's table analysis mode; empty when none or for any other device")
         (table-engine :string :doc "A Filter Table's engine; empty for any other device")
         (table-data-key :string :doc "A Filter Table's prepared table (a table viewer's data key); empty while none is prepared")
         (ir-name :string :doc "A Convolution Reverb's impulse response (No IR when none); empty for any other device")
         ;; A sampler's media (a track's sampler instrument, a sampler rack
         ;; slot); any other device reads the defaults each doc names.
         ;; Computed only while observed: by value, an unobserved one reads
         ;; its last value, the defaults before it was ever observed.
         (sample-buffer :any :doc "The sample a waveform draws (its :buffer); nil when none is loaded")
         (sample-duration :number :doc "The sample's length in seconds; 1 when none is loaded")
         (start-time :number :doc "The playback start shown (start at the displayed step), in seconds; 0 when no sampler")
         (end-time :number :doc "The playback end shown, in seconds; 0 when no sampler")
         (slices (list-of :number) :doc "The slice markers in slice mode, in seconds (every candidate); empty otherwise")
         (slice-active (list-of :number) :doc "Per slice marker, 1 where the slice sensitivity keeps it, else 0 (a waveform's :slice-active)")
         (onsets (list-of :number) :doc "The analysis' onsets, in seconds; empty until ready")
         (analysis-status :string :doc "The sample analysis: none, pending, ready or failed")
         (analysis-message :string :doc "What the analysis says (Analyzing..., 120.0 BPM, the failure); empty when none")
         (analysis-bpm :number :doc "The detected tempo; 0 until ready")
         (analysis-confidence :number :doc "How sure the tempo is, 0-1; 0 until ready")
         (downbeat-time :number :doc "The first downbeat, in seconds; -1 when none")))

;; A fixed modulation source of an instrument (its descriptor's modulators):
;; (nth d.modulators 0).
(def-kind modulator
  :key (device index)
  :host ((device device)
         (index  :int)
         (slot   :int    :doc "The modulation source (1-4) it is")
         (label  :string :doc "The name the panel shows")))

;; A process class of the library (process-library.classes): what
;; (add-process! t c) adds.
(def-kind process-class
  :key (index)
  :host ((index       :int    :doc "Position in the library, from 0")
         (name        :string :doc "The class name (def-process)")
         (doc         :string)
         (source-path :string :doc "The file defining it; empty when none")
         (target      :string :doc "Where its ports write, as the library lists them")
         (lane-count  :int    :doc "Its lane inlets (per-step values)")
         (ports       (list-of :string) :doc "Its port names")))

;; The process library.
(def-kind process-library
  :key ()
  :host ((classes (list-of process-class) :doc "The library's classes (compiled expr bodies excluded)")))

;; One process of a track's chain (t.processes): the project lanes every
;; track runs first, then the track's own. Keyed by its stable id (proc-id),
;; so a reorder keeps the instance; a project lane is a process of every
;; track (its lanes, inlets and bindings fork per track).
(def-kind process
  :key (track proc-id)
  :host ((track         track  :doc "The track whose chain runs it")
         (proc-id       :int   :doc "The host's stable process instance id (cell.pid is a pattern's)")
         (index         :int   :doc "Position in the track's chain, from 0 (fire order)")
         (class         process-class :doc "Its class in the library; nil when the library lacks it, and for an expr card's compiled body (expr#…, never a library class)")
         (class-name    :string)
         (name          :string :doc "The name shown: its instance name, else its class")
         (instance-name :string :doc "Its own name (prob, grab 2, …); empty when none")
         (project       :bool  :doc "A project lane (shared by every track)")
         (default-lane  :bool  :doc "One of the default project lanes")
         (roster        :bool  :doc "A lane the user added to this track")
         (enabled       :bool  :set set-process-enabled!
                        :doc "Runs (false bypasses it; on a project lane, for this track only: set-process-enabled! with :all for every track)")
         (doc           :string :doc "Its class's doc (class.doc, also when class is nil)")
         (source-path   :string :doc "Its class's file (class.source-path, also when class is nil); empty when none")
         (target        :string :doc "Where its class's ports write (class.target, also when class is nil)")
         (lanes         (list-of lane) :doc "Its lane inlets (per-step values), in class order")
         (inlets        (list-of inlet) :doc "Its other numeric inlets")
         (ports         (list-of port) :doc "Its ports, in class order")
         (in-ports      (list-of :string) :doc "The inlets the patchbay shows as in ports: lanes, gates and wired inlets, in class order")
         (cells         (list-of state-cell) :doc "Its class's state cells (the scope)")
         (expr          :bool  :doc "An expr card")
         (expr-line     :string :doc "An expr card's body as one line; empty otherwise")
         (compile-error :string :doc "Why an expr card's body has no compiled class; empty otherwise")
         (error         :string :doc "Why its latest run on this track failed; empty when it did not")))

;; A process's lane: per-step values (t.lanes lists every lane of the track's
;; chain, in the lane selector's order). (set-lane-steps! l steps v) edits it.
(def-kind lane
  :key (process index)
  :host ((process     process :doc "The process whose lane inlet it is")
         (track       track)
         (index       :int    :doc "Position among its process's lanes (p.lanes), from 0")
         (position    :int    :doc "Position in t.lanes, from 0")
         (inlet       :string :doc "The lane inlet's name")
         (label       :string :doc "The selector's long label")
         (short-label :string :doc "The selector's short label (a default lane's name)")
         (type        :string :doc "float, int, gate, track, field, any or enum")
         (min         :number :doc "The values' range (a slot's lo/hi when it has them)")
         (max         :number)
         (default     :number :doc "A step's value when none is set")
         (decimals    :int)
         (forked      :bool   :doc "A project lane this track has its own values for")
         (values      (list-of :number) :doc "One value per step slot (256)")))

;; A process's numeric inlet (not a lane): (nth p.inlets 0).
(def-kind inlet
  :key (process index)
  :host ((process  process)
         (index    :int    :doc "Position in p.inlets, from 0")
         (name     :string)
         (type     :string :doc "float, int, gate, track, field, any or enum")
         (options  (list-of :string) :doc "An enum inlet's labels, by value; empty otherwise")
         (value    :number :set set-inlet!
                   :doc "Its value on this track (an enum's option index; set-inlet! with :all for every track)")
         (default  :number :doc "The class default")
         (min      :number :doc "Its range (the class's, else a hint around value)")
         (max      :number)
         (decimals :int)
         (doc      :string)))

;; A process's port: where it writes. (bind-port! pt x), (add-fanout! pt x),
;; (unbind-port! pt), (clear-port! pt).
(def-kind port
  :key (process index)
  :host ((process      process)
         (index        :int    :doc "Position in p.ports, from 0")
         (name         :string)
         (label        :string :doc "default for the unnamed default port, else name")
         (hint         :string :doc "The class's target hint; empty when none")
         (target       :string :doc "What it writes, as the strip labels it (unbound when nothing)")
         (status       :string :doc "bound (a binding), hint (follows the class hint) or unbound")
         (manual       :bool   :doc "Has its own binding")
         (disconnected :bool   :doc "Disconnected outright: writes nothing (clear-port! reconnects)")
         (mappable     :bool   :doc "Takes parameter targets (a param, a send, a step param)")
         (connectable  :bool   :doc "Takes another process's inlet (a lane or an inlet)")
         (bindable     :bool)
         (target-kind  :string :doc "The kind of target it takes; empty for any")
         (target-process process :doc "The process its binding wires into (same layer), or nil")
         (target-inlet :string :doc "That process's inlet; empty when not wired")
         (target-step-param :string :doc "The step param it is bound to; empty when none")
         (fanout       (list-of fanout) :doc "Its extra scaled targets")))

;; An extra scaled target of a port: (nth pt.fanout 0). Positional: removing
;; one retargets the handles after it.
(def-kind fanout
  :key (port index)
  :host ((port   port)
         (index  :int    :doc "Position in pt.fanout, from 0")
         (target :string :doc "What it writes, as the strip labels it")
         (target-process process :doc "The process inlet it wires into (same layer), or nil")
         (target-inlet :string)
         (target-step-param :string)
         (lo     :number :set (fanout-setter "fanout-lo") :doc "The target value at the port's low end")
         (hi     :number :set (fanout-setter "fanout-hi") :doc "… and at its high end")))

;; A process's state cell (its scope): (nth p.cells 0).
(def-kind state-cell
  :key (process index)
  :host ((process process)
         (index   :int    :doc "Position in p.cells, from 0")
         (name    :string)
         (values  (list-of :number) :doc "Its history on the process's track, one sample per fire (64 at most); empty before the first")))

;; A param of a graph node or edge: (graph-param-named n "threshold"), (nth
;; e.params 0). Keyed by its owner and its name: a re-evaluated prototype
;; that reorders its :params keeps each handle on its param, and one that
;; renames or drops a param leaves that handle stale (never another param's).
;; Registered on the first read of n.params / e.params.
(def-kind graph-param
  :key ((graph-node graph-edge) pname)
  :host ((node    graph-node :doc "The node whose param it is; nil for an edge's")
         (edge    graph-edge :doc "The edge whose param it is; nil for a node's")
         (index   :int    :doc "Position in its owner's params, from 0")
         (name    :string)
         (type    :string :doc "float or int")
         (min     :number)
         (max     :number)
         (default :number :doc "The prototype's default")
         (value   :number :set set-graph-param-value
                  :doc "The current scene's value (a finite number in min to max; an int param an integer)")))

;; One edge of a graph, from its node: (nth n.edges 2), (graph-edge-to n m).
;; Keyed by its source node and its target's index; registered on the first
;; read of n.edges.
(def-kind graph-edge
  :key (graph-node index)
  :host ((from    graph-node :doc "The node it leaves")
         (to      graph-node :doc "The node it reaches")
         (params  (list-of graph-param) :doc "The edge set's params (weight, dampening, …)")))

;; One node of a graph: (nth g.nodes 3). Positional (the model's nodes are
;; numbered): a node-count change adds or drops the last ones.
(def-kind graph-node
  :key (graph index)
  :host ((graph      graph   :doc "The graph it belongs to")
         (index      :int    :doc "Node index, from 0")
         (resolution :string :set (graph-node-setter "resolution")
                     :doc "Its step resolution (the first of its cycle), one of graph-timebase-options")
         (resolution-cycle (list-of :string) :set (graph-node-setter "resolution-cycle")
                     :doc "Its round-robin resolutions, one per fire (graph-timebase-options)")
         (quantize   :string :set (graph-node-setter "quantize")
                     :doc "Its quantize grid (the first of its cycle), one of graph-quantize-options")
         (quantize-cycle (list-of :string) :set (graph-node-setter "quantize-cycle")
                     :doc "Its round-robin quantize grids, one per fire; (off) for none")
         (delay      :int    :set (graph-node-setter "delay") :doc "Propagation delay in steps (0 or more)")
         (route      track   :set set-graph-node-route
                     :doc "The track it plays (a rack-owned graph's member track); nil while off or gating a generator")
         (generator  :int    :doc "The generator (a jaki instance id) its fires gate or restart, -1 for none (gate-generator!)")
         (restart    :bool   :doc "Its fires restart the generator rather than gate it")
         (seed-route :bool   :set (graph-node-setter "seed-route")
                     :doc "Seeded by its route track (false: by seeds only)")
         (seeds      (list-of track) :set set-graph-node-seeds
                     :doc "The tracks that seed it (its route track while seed-route); setting them stops following the route")
         (seed-on-reset :number :set (graph-node-setter "seed-on-reset")
                     :doc "Energy it starts with at a reset boundary (0 or more)")
         (group      :int    :range (0 3) :set (graph-node-setter "group")
                     :doc "Its neural group, 0-3 (A-D)")
         (params     (list-of graph-param) :doc "Its behavioral params, in prototype order")
         (edges      (list-of graph-edge) :doc "Its outgoing edges, by target index")
         (sounding   (list-of (list-of :number))
                     :doc "The notes it sounds now, (note velocity) per open gate, oldest first; empty while stopped")))

;; A graph-mode sequencer: a created kind's instance (neural, …) or a
;; script's def-sequencer. (graph-of self) is an instance's.
(def-kind graph
  :key (index)
  :host ((index      :int    :doc "Position in project.graphs")
         (gid        :int    :doc "Its sequencer id: the created instance's id (self.id) for an instance's graph")
         (name       :string)
         (owner      group   :doc "The drum rack that owns it (its routes are the rack's members), or nil")
         (variable   :bool   :doc "Its node count can change")
         (min-nodes  :int)
         (max-nodes  :int    :doc "Its capacity")
         (node-count :int    :set (graph-setter "node-count")
                     :doc "Active nodes (min-nodes to max-nodes; a variable graph's only)")
         (reset-bars :number :set (graph-setter "reset-bars") :doc "Reset period in bars, 0 for none")
         (max-poly   :int    :set (graph-setter "max-poly") :doc "Fires kept per boundary (0 or more)")
         (max-poly-selection :string :set (graph-setter "max-poly-selection")
                     :doc "Which fires survive past max-poly, one of graph-max-poly-selection-options")
         (group-trace-decay :number :range (0 1) :set (graph-setter "group-trace-decay"))
         (group-coupling-scale :number :range (0 2) :set (graph-setter "group-coupling-scale"))
         (group-excite-floor :number :range (0 1) :set (graph-setter "group-excite-floor"))
         (group-gain (list-of :number)
                     :doc "Propagation gain between neural groups, 4x4 row-major: cell (+ (* row 4) col), source row, target column; 0-2; set-group-gain!")
         (group-coupling (list-of :number)
                     :doc "Activity coupling between neural groups, 4x4 row-major, -2 to 2; set-group-coupling!")
         (nodes      (list-of graph-node) :doc "Its active nodes")
         ;; Playback (the scheduler's visualization of it).
         (active     :bool   :doc "The scheduler runs it")
         (beat       :number :doc "Its current beat")
         (energy     (list-of :number) :doc "Each node's energy, 0-4")
         (triggers   (list-of :number) :doc "Each node's trigger activity, 0-1")
         (dampening  (list-of (list-of :number)) :doc "Each edge's live dampening, by from row and to column, 0-1")))

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
         (in-selection :bool :doc "The current track or one of selection.tracks (the multi-selection highlight)")
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
         (instrument-id :string :doc "The instrument it plays, as the browser's Instruments tab names it (builtin:sampler, …); empty for an empty track or a drum rack")
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
         (variants  (list-of variant) :doc "The track's p-lock variants, by label (stamp-variant!)")
         ;; Process lanes.
         (processes (list-of process) :doc "The process chain, in fire order: the project lanes, then the track's own")
         (lanes     (list-of lane) :doc "Every lane of the chain, in the lane selector's order")
         (active-notes (list-of (list-of :number))
                    :doc "The notes sounding now, (note velocity trigger-id) per note, ascending; a piano-keyboard's :notes-by-track takes the rows")))

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
         (delete-target :bool :set set-group-delete-target
                    :doc "The mixer's delete target (Backspace deletes or ungroups it)")
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
         (bid     :int    :doc "The bank's stable id (the scene-bank commands' :bank-id)")
         (name    :string :doc "The bank's own name; empty when it has none")
         (label   :string :doc "A, B, … with the bank name after a dash when it has one")
         (scenes  (list-of scene))
         (playing :bool   :doc "Holds the playing scene")))

(def-kind transport
  :key ()
  :host ((playing         :bool :set set-transport-playing)
         (recording       :bool :set set-transport-recording)
         (scene           scene :doc "The playing scene")
         (queued          scene :doc "The scene a quantized launch waits for, or nil")
         (launch-quantize :string :set set-transport-launch-quantize
                          :doc "Scene launch quantization: off, 1/16, …, 1 bar")
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

;; ── The browser, the sound palette, the editor and the app's views ──

;; A saved Sound or kit of the browser's Sounds and Kits tabs:
;; (nth browser.sound-presets 0). Positional; the instance follows its file.
(def-kind preset-file
  :key (index)
  :host ((index  :int    :doc "Position in its listing, from 0")
         (type   :string :doc "sound or kit")
         (icon   :string :doc "The browser icon: piano, waveform, sine or sampler")
         (name   :string)
         (path   :string :doc "The file (load-sound-onto-track and load-kit take it)")
         (pads   :int    :doc "A kit's pad count; 0 for a Sound")
         (author :string)
         (tags   (list-of :string))))

;; A drum rack slot's presets, for the browser's Presets tab: (nth
;; browser.rack-slots 0). The instance follows its slot device.
(def-kind slot-presets
  :key (index)
  :host ((index  :int    :doc "The slot's position in the rack, from 0")
         (device device  :doc "The rack slot device; nil until the device sync has it")
         (instrument :string :doc "The slot's instrument")
         (instrument-label :string :doc "Its display name")
         (presets (list-of :string) :doc "The presets it can load")
         (user-presets (list-of :string) :doc "The ones the user saved (listed under Library)")
         (preset :string :doc "Its loaded preset; empty when none")))

;; The browser sidebar: what the shown track plays and can load, the saved
;; Sounds and kits, the sample preview.
(def-kind browser
  :key ()
  :host ((track track :doc "The track the sidebar shows (the current track)")
         (instrument-kind :string :doc "sampler, instrument or empty")
         (instrument :string :doc "Its instrument (a drum rack track's name); empty for a sampler")
         (instrument-label :string :doc "The instrument's display name")
         (preset :string :doc "Its loaded preset; empty when none")
         (presets (list-of :string) :doc "The presets it can load (a drum rack's: the rack presets)")
         (user-presets (list-of :string) :doc "The ones the user saved (listed under Library)")
         (sample :string :doc "A sampler's sample file; empty otherwise")
         (rack-slots (list-of slot-presets) :doc "A drum rack's slots, with their presets")
         (engines (list-of :string) :doc "The instrument engines the project uses")
         (sound-presets (list-of preset-file) :doc "The saved Sounds")
         (kit-presets (list-of preset-file) :doc "The saved kits")
         (library-epoch :int :doc "Moves when the instrument or effect library changes on disk; read it to re-list a library tree")
         (preview-playing :bool :doc "A sample preview plays")
         (preview-position :number :doc "The preview's position in seconds; 0 while stopped")))

;; A sound (a patch of a track's pool) as the sound palette lists it:
;; (nth sound-palette.sounds 0). Keyed by its patch id.
(def-kind sound
  :key (track patch-id)
  :host ((track     track  :doc "The track whose pool holds it")
         (patch-id  :int   :doc "The host's stable patch id")
         (mix-id    :int   :doc "The mix its first use pairs it with; -1 when unknown")
         (name      :string :set set-sound-name :doc "Non-empty; one undo entry")
         (referents :string :doc "Where it is used: Scene 1, Pattern 5, …; unused for a library orphan")
         (referents-short :string :doc "The same, abbreviated: S1 P5 T2")
         (base      :bool  :doc "The scene-effective sound")
         (track-sound :bool :doc "The track's own sound")
         (current   :bool  :doc "The palette target's current sound")
         (preset    :string :doc "The preset it was loaded from (* when edited since); empty when unknown")
         (sample    :string :doc "A sampler sound's sample name; empty otherwise")
         (diff-up   :int   :doc "Params higher than the current sound's")
         (diff-down :int   :doc "Params lower than the current sound's")
         (colored   :bool  :doc "It has a palette color")
         (color     :rgb   :doc "Its palette color, themed; the timeline's gray without one")
         (glyph-key :string :doc "The sound-glyph source key of its glyph")))

;; The sound palette overlay: (open-sound-palette! t), (apply-sound! s).
(def-kind sound-palette
  :key ()
  :host ((open       :bool   :doc "The overlay is open")
         (track      track   :doc "The track whose sounds it shows; nil while closed")
         (target     :string :doc "What apply and fork act on: take, pattern or cell")
         (target-id  :int    :doc "The take or pattern id; -1 for cell")
         (instrument :string :doc "The track's instrument, for the header")
         (sounds     (list-of sound))))

;; A defmacro of the patch editor's macro sidebar: the patch's own, or the
;; saved library's. Positional; the instance follows its name.
(def-kind editor-macro
  :key (index)
  :host ((name    :string)
         (library :bool   :doc "A saved library macro (else the patch's own)")
         (params  (list-of :string))
         (calls   (list-of :string) :doc "The macros its body calls (a library macro's imports)")
         (outputs (list-of :string) :doc "A library macro's outputs")
         (summary :string :doc "A library macro's summary")
         (used    :bool   :doc "A library macro the patch imports")))

;; A file-backed tensor asset the patch can use. The instance follows its
;; reference.
(def-kind editor-asset
  :key (index)
  :host ((index       :int    :doc "Position in editor.assets, from 0")
         (reference   :string :doc "What a tensor node names")
         (tier        :string :doc "draft, user or factory")
         (source-path :string)))

;; The selected file-backed tensor node's asset (editor.selected-asset).
(def-kind asset-info
  :key ()
  :host ((reference   :string)
         (tensor-kind :string :doc "Its declared kind; empty when none")
         (layout      :string :doc "Its declared layout; empty when none")
         (shape       (list-of :int))
         (source      :string :doc "Where it came from; empty when unknown")
         (wave-count  :int)
         (waves-per-set :int  :doc "0 when it declares no grouping")
         (set-count   :int)
         (sets        (list-of :string) :doc "The set labels it declares")
         (wave-names  (list-of :string) :doc "The wave names it declares")))

;; The instrument and effect editor.
(def-kind editor
  :key ()
  :host ((mode      :string :doc "Empty, new-instrument, edit-instrument, new-effect or edit-effect")
         (surface   :string :doc "patch or code")
         (buffer    :string :doc "The edited buffer's name")
         (error     :string :doc "The last compile or save error; empty when none")
         (canceling :bool)
         (run-mode  :string :set set-editor-run-mode :doc "A draft instrument's run mode: instrument or free_patch")
         (active-macro :string :doc "The macro view's macro a library action applies to")
         (active-macro-action :string :doc "save-to-library, fork or empty")
         (open-macro :string :doc "The macro view open in the patcher; empty at the root")
         (patch-macros (list-of editor-macro) :doc "The patch's own defmacros")
         (library-macros (list-of editor-macro) :doc "The saved defmacro library")
         (assets    (list-of editor-asset))
         (selected-asset asset-info :doc "The selected file-backed tensor node's asset, or nil")))

;; Patch Learn's plan, epoch and result rows: (nth learn.plan-params 0).
;; Positional.
(def-kind learn-plan-param
  :key (index)
  :host ((index  :int)
         (name   :string)
         (status :string :doc "learnable, frozen or unsupported")
         (reason :string :doc "Why it is frozen or unsupported")))
(def-kind learn-epoch-param
  :key (index)
  :host ((index  :int)
         (name   :string)
         (from   :number :doc "The seeded value")
         (value  :number :doc "The value at the latest epoch")
         (change :number)
         (step   :number)))
(def-kind learn-delta
  :key (index)
  :host ((index  :int)
         (name   :string)
         (from   :number)
         (to     :number)
         (change :number)))

;; Patch Learn: the target, the training settings, progress and the result.
(def-kind learn
  :key ()
  :host ((target-path :string :doc "The target sample; empty when none")
         (target-name :string)
         (phase :string :doc "pick, planning, configure, training, result or error")
         (method :string :set (learn-setter "method") :doc "One of learn-method-options")
         (epochs :int :range (1 2000) :set (learn-setter "epochs"))
         (cma-generations :int :range (1 1000) :set (learn-setter "cma-generations"))
         (cma-population :int :range (0 4096) :set (learn-setter "cma-population") :doc "0 for auto, else at least 4")
         (cma-sigma :number :range (0 10) :set (learn-setter "cma-sigma") :doc "Above 0")
         (cma-seed :int :range (0 4294967295) :set (learn-setter "cma-seed"))
         (cma-forward-batch :int :range (0 4096) :set (learn-setter "cma-forward-batch") :doc "0 for auto")
         (local-epochs :int :range (0 2000) :set (learn-setter "local-epochs"))
         (cma-continue :int :range (0 4096) :set (learn-setter "cma-continue"))
         (cma-refine-epochs :int :range (0 2000) :set (learn-setter "cma-refine-epochs"))
         (cma-refine-mode :string :set (learn-setter "cma-refine-mode") :doc "One of learn-refine-mode-options")
         (cma-final-epochs :int :range (0 2000) :set (learn-setter "cma-final-epochs"))
         (pitch-hz :number :set (learn-setter "pitch-hz") :doc "The target's pitch; 0 until known")
         (gate-frames :int :set (learn-setter "gate-frames") :doc "The note length in frames; 0 until known")
         (stage :string)
         (current-epoch :int)
         (total-epochs :int)
         (loss :number)
         (losses (list-of :number) :doc "Per epoch")
         (optimization-losses (list-of :number) :doc "Per optimizer iteration")
         (plan-params (list-of learn-plan-param))
         (epoch-params (list-of learn-epoch-param))
         (improvement-pct :number)
         (abs-distance :number)
         (basin-check :string)
         (result-deltas (list-of learn-delta))
         (seeded-wav :string)
         (final-wav :string)
         (applied :bool :doc "The result was applied to the instrument")
         (error :string)))

;; A frozen MIDI capture's lane (one per played track and pitch) and note.
(def-kind retro-lane
  :key (index)
  :host ((index :int)
         (label :string :doc "Track · pitch")))
(def-kind retro-item
  :key (index)
  :host ((index :int)
         (lane  retro-lane)
         (start :number :doc "Seconds into the capture")
         (end   :number)))

;; MIDI capture (Capture MIDI): the frozen live history and its audition.
(def-kind retro
  :key ()
  :host ((lanes     (list-of retro-lane))
         (items     (list-of retro-item))
         (duration  :number :doc "Seconds")
         (truncated :bool   :doc "The history ran past its window")
         (error     :string)
         (playing   :bool   :doc "An audition plays")
         (position  :number :doc "The audition's position in its loop, 0 to 1")
         (playhead  :number :doc "Where the audition plays, seconds into the capture; -1 idle")))

;; The song export modal (export is a module form, hence song-export).
(def-kind song-export
  :key ()
  :host ((default-name :string :doc "The suggested file name")
         (project      :string :doc "The saved project it exports")
         (folder       :string :doc "The recordings folder")
         (end          :number :doc "The arrangement's end beat")
         (busy         :bool   :doc "An export runs")
         (done         :bool   :doc "The last export completed")
         (message      :string)
         (percent      :number :doc "Render progress; -1 while not rendering")
         (output-name  :string :doc "The file being written")
         (reveal-label :string :doc "Show in Finder, or Open folder")))

;; A MIDI input: (nth settings.midi-devices 0). The instance follows its
;; device id.
(def-kind midi-device
  :key (index)
  :host ((index     :int)
         (device-id :string :doc "The service's stable device id")
         (name      :string)
         (enabled   :bool   :set set-midi-device-enabled)
         (connected :bool)
         (status    :string)))

;; Settings: the audio workers and the MIDI inputs.
(def-kind settings
  :key ()
  :host ((audio-workers-choice :string :set set-audio-workers-choice
                               :doc "One of project.audio-workers-options; applies at the next launch")
         (audio-workers-note :string :doc "What runs now and after a restart")
         (midi-devices (list-of midi-device))
         (midi-error :string)
         (midi-persistent :bool :doc "Device choices are saved")))

;; The agent.
(def-kind agent
  :key ()
  :host ((generation :int :doc "Moves whenever an agent session changes")))

;; A note of the piano roll's source: (nth piano-roll.notes 0). The host gives
;; each note an id while it exists; a set! that moves a note keeps its id (and
;; its handle), and one moved onto another replaces it (the other's handle goes
;; stale). An edit made otherwise (the legacy piano roll, the step grid,
;; recording) keeps a note's handle while the note stays where it was; an undo
;; or redo that changes the notes makes every note handle stale. A set! or
;; delete of a stale note is an error ("the note is gone").
(def-kind note
  :key (track nid)
  :host ((track    track  :doc "The track whose source holds the note (the piano roll's track)")
         (nid      :int   :doc "The host's id for the note while it exists")
         (pitch    :int   :range (-48 48) :set (note-setter "pitch")
                   :doc "Semitones from C4 (the track's root); the lane is pitch-max minus pitch")
         (start    :number :set (note-setter "start")
                   :doc "Onset in steps on the source's axis: its step plus its offset into it")
         (length   :number :range (0.03125 32) :set (note-setter "length") :doc "Duration in steps")
         (velocity :number :range (0 1) :set (note-setter "velocity")
                   :doc "Its step's velocity: a chord's notes share it; a note keeps its own on a step it moves to alone")
         (selected :bool   :set (note-setter "selected") :doc "Selected in the piano roll (no undo entry)")
         (label    :string :doc "Its pitch name, with its offset when off the step: C4, D#3 +0.50")
         (hidden   :bool   :doc "A script drag's note lies over it: unlisted until the drag ends (or moves on); its set! and delete are errors meanwhile")))

;; The piano roll: what the current track's note editor edits (its edit
;; focus) and the notes there. Model fields but playhead.
(def-kind piano-roll
  :key ()
  :host ((track      track  :doc "The track it edits (the current track)")
         (focus-kind :string :doc "Where its edits land: live (the playing pattern), pattern (a pinned pattern) or take (a pinned take)")
         (clip-kind  :string :doc "The pinned arrangement clip's source: pattern or take; none in follow mode")
         (clip       clip   :doc "The pinned arrangement clip (clip.start, end, offset: the clip panel), or nil")
         (focus-label :string :doc "The header's source name: Pattern 3 (scene), Pattern 5 — 2 clips, a take's name")
         (focus-num-steps :int :doc "The source's length in steps (a take's playable length)")
         (window-marker :number :doc "The pinned clip's start in its source, in steps; -1 without one")
         (window-span (list-of :number) :doc "(start end) of the window the clip plays when shorter than its source; empty otherwise")
         (window-repeat :number :doc "How many times the clip plays its source over, when more than once; 0 otherwise")
         (playhead   :number :doc "The playing step on the source's axis; -1 while hidden (a pinned source the song does not play)")
         (notes      (list-of note) :doc "The source's notes, by step, then pitch and offset; (add-note! …), (delete-notes! …)")))

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
         (step-param-options (list-of :string) :doc "The step params a process port may write (bind-port! pt name)")
         (groove-pool (list-of pool-groove) :doc "The project's grooves, in pool order")
         (groove-library (list-of library-groove) :doc "The groove files of the library, factory first")
         (macros (list-of macro) :doc "The project's macros, in macro order")
         (name :string :doc "The project's name; empty while unsaved")
         (audio-workers-options (list-of :string) :doc "The audio worker choices, for settings.audio-workers-choice")
         (graphs (list-of graph) :doc "The graph-mode sequencers, in publish order")
         (instances (list-of :any)
                    :doc "The project's package instances, as seq-package-tree takes them: dicts :id :kind :label :owner-label :owner-rack (a rack's group id, nil for the project) :registered? (false: its kind is not loaded)")))

;; ── Collections and actions ──

(def tracks () project.tracks)
(def scenes () project.scenes)
(def banks () project.banks)
(def buses () project.buses)
(def groups () project.groups)
(def routes () project.routes)
(def macros () project.macros)
(def graphs () project.graphs)

;; Graph sequencers (spec §14.2k). The graph of a created instance (self) or
;; of a sequencer id, or nil.
(def graph-of (x)
  (let ((id (if (number? x) x x.id)))
    (first (filter (lambda (g) (= g.gid id)) project.graphs))))
;; A node's or an edge's param named name, or nil.
(def graph-param-named (x name)
  (first (filter (lambda (p) (= p.name name)) x.params)))
;; Node n's edge to node m (an instance or an index), or nil.
(def graph-edge-to (n m)
  (let ((index (if (number? m) m m.index)))
    (first (filter (lambda (e) (= e.to.index index)) n.edges))))
;; Set cell (row col) of graph g's group gain (0-2) or coupling (-2 to 2)
;; matrix: rows are the source group, columns the target group, 0-3; the
;; cell is (nth g.group-gain (+ (* row 4) col)).
(def set-group-gain! (g row col v) (graph-edit g "group-gain" v :row row :col col))
(def set-group-coupling! (g row col v) (graph-edit g "group-coupling" v :row row :col col))
;; Make node n's fires gate generator id (a generator instance, such as a
;; jaki, of the graph's owner, never the graph's own instance) instead of
;; playing a note; :restart true restarts it each fire.
;; (set! n.route t) routes it back to a track.
(def gate-generator! (n id &key (restart false))
  (graph-edit n.graph "generator" id :node n.index :restart restart))
;; graph-node.resolution / quantize labels (the host checks they match its
;; own), graph.max-poly-selection's.
(def graph-timebase-options '("1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh"))
(def graph-quantize-options (cons "off" graph-timebase-options))
(def graph-max-poly-selection-options
  '("deterministic" "propagation" "random" "markov" "loudest" "lowest-transpose" "highest-transpose" "seed-first"))

;; The transport's launch quantization; "off" until the host publishes it.
(def launch-quantize ()
  (let ((q transport.launch-quantize))
    (if (or (= q nil) (= q "")) "off" q)))

;; Launch a scene with the transport's launch quantization.
(def launch! (s)
  (host-command "switch-pattern" (dict :idx s.index :quantize (launch-quantize))))

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
    (dict :track-id c.track.tid :pattern-id c.pid :quantize (launch-quantize))))

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

;; P-lock rack slot d's strip control field (gain, pan, muted, soloed,
;; base-note or voices) to v on steps, a list of step instances of d's track
;; (any other track's step is an error); one undo entry. Steps already locked
;; to v are left alone. v follows the field's own range: gain 0-2, pan -1-1,
;; base-note -48-48, voices an integer 1-16, a bool for muted and soloed.
(def lock-strip! (d field steps v)
  (host-command "set-device-strip-locks"
    (merge (device-target d)
           :field field :steps (map (lambda (s) s.index) steps)
           :step-tracks (map (lambda (s) s.track.tid) steps) :value v)))

;; Clear rack slot d's strip control field's p-locks on steps; one undo entry.
(def unlock-strip! (d field steps)
  (host-command "clear-device-strip-locks"
    (merge (device-target d)
           :field field :steps (map (lambda (s) s.index) steps)
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
    (dict :group-id rc.group.gid :clip-id rc.cid :quantize (launch-quantize))))

;; Silence rack g in the current scene.
(def silence-rack! (g)
  (host-command "launch-rack-clip"
    (dict :group-id g.gid :clip-id 0 :quantize (launch-quantize))))

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

;; ── Process lanes ──

;; Set lane l to v on steps (step instances of l's track); a drag's set!s
;; join one undo entry.
(def set-lane-steps! (l steps v)
  (edit-process (process-target l.process) "lane-steps" :inlet l.inlet :value v
                :steps (map (lambda (s) s.index) steps)
                :step-tracks (map (lambda (s) s.track.tid) steps)))

;; Move p before process before of its track and layer (nil: to the end of
;; its layer). Moving a project lane moves it on every track.
(def move-process! (p before)
  (edit-process (process-target p) "move"
                :before (if before before.proc-id nil)
                :before-track-id (if before before.track.tid nil)))

;; Add a process of class c (a process-class) to track t's own lanes.
(def add-process! (t c)
  (edit-process (dict :track-id t.tid) "add" :class c.name))

;; Remove p from its track (a lane the user added, from every scene).
(def remove-process! (p)
  (edit-process (process-target p) "remove"))

;; A port's target: a param of the track's devices, a send of the track, a
;; lane or inlet of another process of the track, or a step param's name
;; (one of project.step-param-options).
(def port-target (x)
  (if (string? x)
    (dict :kind "step-param" :param x)
    (match x.kind
      "eseq.kinds:param" (merge (device-target x.device) :kind "param" :param-idx x.index)
      "eseq.kinds:send" (dict :kind "bus-send" :bus-id x.bus.bid :track-id x.track.tid)
      "eseq.kinds:lane" (dict :kind "inlet" :proc-id x.process.proc-id :inlet x.inlet
                              :track-id x.process.track.tid)
      "eseq.kinds:inlet" (dict :kind "inlet" :proc-id x.process.proc-id :inlet x.name
                               :track-id x.process.track.tid)
      _ (seq-error (str "not a port target: " x)))))

;; Bind port pt to x (port-target), replacing its binding.
(def bind-port! (pt x &key (all false))
  (edit-process (port-address pt) "bind" :target (port-target x) :all all))

;; Add x as an extra scaled target of pt (at the port's own range).
(def add-fanout! (pt x &key (all false))
  (edit-process (port-address pt) "add-fanout" :target (port-target x) :all all))

;; Disconnect pt: it writes nothing (its binding and class hint muted).
(def unbind-port! (pt &key (all false))
  (edit-process (port-address pt) "unbind" :all all))

;; Drop pt's own binding (it follows its class hint again).
(def clear-port! (pt &key (all false))
  (edit-process (port-address pt) "clear" :all all))

;; Remove fan-out entry fo.
(def remove-fanout! (fo &key (all false))
  (edit-process (fanout-address fo) "remove-fanout" :all all))

;; ── The piano roll ──

;; note.pitch's range: semitones from C4.
(def pitch-min -48)
(def pitch-max 48)

;; Add a note to the piano roll's source at start (steps), pitch, length
;; (steps); velocity nil keeps the step's. One undo entry; a note already
;; there (same step, pitch and offset) is replaced. Returns nil: the note
;; shows in piano-roll.notes after the next tick.
(def add-note! (start pitch length &key (velocity nil))
  (host-command "add-note"
    (dict :track-id piano-roll.track.tid :start start :pitch pitch :length length
          :velocity velocity)))

;; Delete notes, all of one track (else an error that deletes none); one undo
;; entry. A note gone (or hidden under a drag) is an error that deletes none.
(def note-track-id (n)
  (let ((t n.track))
    (if t t.tid nil)))
(def delete-notes! (notes)
  (if (empty? notes)
    nil
    (host-command "delete-notes"
      (dict :track-id (note-track-id (first notes))
            :track-ids (map note-track-id notes)
            :nids (map (lambda (x) x.nid) notes)))))

;; ── The sound palette and Patch Learn ──

;; Open the sound palette on track t's sounds; target is take, pattern or
;; cell (the track's bound sound when nil), id the take or pattern id.
(def open-sound-palette! (t &key (target nil) (id nil))
  (host-command "sound-palette-open"
    (if target
      (dict :track-id t.tid :target-kind target :target-id id)
      (dict :track-id t.tid))))
(def close-sound-palette! () (host-command "sound-palette-close" (dict)))

;; Make sound s what the palette's target plays (by reference: later edits
;; to s follow). One undo entry.
(def apply-sound! (s)
  (host-command "sound-apply" (dict :track-id s.track.tid :patch s.patch-id)))

;; Apply s with the mix its first use pairs it with (none: an error).
(def apply-sound-with-mix! (s)
  (host-command "sound-apply-with-mix"
    (dict :track-id s.track.tid :patch s.patch-id :mix s.mix-id)))

;; Give track t's palette target its own copy of its current sound.
(def fork-sound! (t) (host-command "sound-fork" (dict :track-id t.tid)))

;; Patch Learn's methods and shortlist execution modes (the host checks they
;; match its own).
(def learn-method-options
  '("Local fit + basin check" "Evolutionary search only" "Evolutionary search + training"))
(def learn-refine-mode-options '("Batched" "Scalar" "Auto"))
