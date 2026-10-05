;; eseq.kinds — the host kinds: what a view can be built from
;; (docs/kind-bindings-spec.md §3.4, §4, §9, §14).
;;
;; Each kind here is a projection of the sequencer's own state. The host
;; registers the instances (tracks, scenes, banks, buses, groups, devices,
;; sends; steps on first read of `t.steps`, params on first read of
;; `d.params`), pushes their `:host` fields,
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
        tracks scenes banks buses groups routes
        launch! clone-scene! delete-scene! step-preset!
        device-param lock-param! unlock-param!
        lock-none lock-seq lock-variant
        reset-tuning! justify-tuning! randomize-tuning! stretch-tuning!
        set-bar-transpose! mod-in-level
        mute-group-options accum-mode-options tuning-root-options tuning-mode-options
        voice-priority-options mono-trigger-options swing-resolution-options
        roll-rate-options)

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
;; A device param's own value (never a p-lock), in display units. Addressed
;; by stable ids (track id, device id) so a reorder before the command lands
;; cannot retarget it.
(def set-param-base (p v)
  (host-command "set-device-param"
    (dict :track-id p.device.track.tid :device p.device.did :param-idx p.index :value v)))

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
         (value   :number :doc "The value shown: on the current track the p-lock at the selected (or playing) step, else the base under any engaged macro")
         (base    :number :set set-param-base
                  :doc "The device's own value (setting it never p-locks; see lock-param!). Set values are clamped; enum and boolean ones rounded (true/false work)")
         (locked  :bool   :doc "value comes from a p-lock")
         (has-locks :bool :doc "Some step of the track's pattern locks this param")
         (text    :string :doc "The option label value selects (on/off for a boolean); empty for continuous params")
         (printing :bool  :doc "Held under a live print latch while playing and recording")))

;; One device of a track's chain: the instrument (slot -1), then effects.
;; Keyed by its stable id: a reorder keeps the instance, only slot moves.
(def-kind device
  :key (track did)
  :host ((track   track   :doc "The track whose chain holds the device")
         (slot    :int    :doc "-1 for the instrument, else the effect slot")
         (did     :int    :doc "The host's stable device id on its track: 0 for the instrument, else the effect's instance id")
         (type    :string :doc "What the device is: the instrument type (synth, sampler, …) or the effect (Filter, Reverb, …)")
         (name    :string :doc "Device name")
         (enabled :bool   :doc "False while bypassed")
         (params  (list-of param) :doc "The device's parameters, in descriptor order")
         (playhead :number :doc "A sampler's playing position in seconds, 0 when idle or not a sampler")))

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
         (devices   (list-of device))
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
         (delete-target :bool :set set-track-delete-target :doc "Among the mixer's delete target (one track or several)")))

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
         (bus       bus     :doc "The group's bus, or nil")))

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
         (output-options (list-of bus) :doc "The buses a track's output may be set to (nil is sends only)")))

;; ── Collections and actions ──

(def tracks () project.tracks)
(def scenes () project.scenes)
(def banks () project.banks)
(def buses () project.buses)
(def groups () project.groups)
(def routes () project.routes)

;; Launch a scene with the transport's launch quantization.
(def launch! (s)
  (host-command "switch-pattern"
    (dict :idx s.index :quantize transport.launch-quantize)))

;; Copy scene s to a new scene at the end of its bank, which then plays.
(def clone-scene! (s) (host-command "clone-pattern" (dict :idx s.index)))

;; Delete scene s (never the last one); the playing scene keeps playing
;; unless it is s.
(def delete-scene! (s) (host-command "delete-pattern" (dict :idx s.index)))

;; step.lock-kind values.
(def lock-none 0)
(def lock-seq 1)
(def lock-variant 2)

;; Device d's param named name, or nil.
(def device-param (d name)
  (first (filter (lambda (p) (= p.name name)) d.params)))

;; P-lock param p to v (display units) on steps, a list of step instances of
;; p's track (any other track's step is an error); one undo entry. Steps
;; already locked to v are left alone.
(def lock-param! (p steps v)
  (host-command "set-device-param-locks"
    (dict :track-id p.device.track.tid :device p.device.did :param-idx p.index
          :steps (map (lambda (s) s.index) steps)
          :step-tracks (map (lambda (s) s.track.tid) steps) :value v)))

;; Clear param p's p-locks on steps (a list of step instances of p's track);
;; one undo entry.
(def unlock-param! (p steps)
  (host-command "clear-device-param-locks"
    (dict :track-id p.device.track.tid :device p.device.did :param-idx p.index
          :steps (map (lambda (s) s.index) steps)
          :step-tracks (map (lambda (s) s.track.tid) steps))))

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
