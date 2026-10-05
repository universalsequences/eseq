;; eseq.kinds — the host kinds: what a view can be built from
;; (docs/kind-bindings-spec.md §3.4, §4, §9).
;;
;; Each kind here is a projection of the sequencer's own state. The host
;; registers the instances (tracks, scenes, banks, devices; steps on first
;; read of `t.steps`), pushes their `:host` fields, and checks at startup that
;; it publishes exactly the fields declared below (crates/sequencer/src/ui/
;; host_kinds.rs). A view imports what it uses:
;;
;;   (import eseq.kinds :refer (track tracks transport scenes banks selection launch!))
;;
;; Read a field by value (`t.name`, re-renders the reader) or bind it
;; (`#'t.volume`, repaints only). Writable fields carry `:set`: `(set! t.volume
;; 0.5)`, `(toggle! t.muted)`, `(toggle! s.active)`, `(set! selection.track t)`.
;; The host computes a field only while something observes it (a reader or a
;; held `#'`); reading an unobserved one asks the host for its value.

(module eseq.kinds)

(export track scene bank transport selection project
        tracks scenes banks launch! clone-scene! delete-scene! step-preset!)

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

;; ── Kinds ──

;; One step slot of a track: (nth t.steps 5). Positional: a scene switch
;; changes no instance, only the values (spec D2).
(def-kind step
  :key (track index)
  :host ((index    :int    :doc "Step index on its track, from 0")
         (track    track   :doc "The track the step belongs to")
         (active   :bool   :set set-step-active :doc "The step triggers")
         (playing  :bool   :doc "The playhead is on this step while the transport runs")
         (selected :bool   :doc "Selected for editing (the selected track's steps only)")))

;; One device of a track's chain: the instrument (slot -1), then effects.
(def-kind device
  :key (track slot)
  :host ((track   track   :doc "The track whose chain holds the device")
         (slot    :int    :doc "-1 for the instrument, else the effect slot")
         (name    :string :doc "Device name")
         (enabled :bool   :doc "False while bypassed")))

(def-kind track
  :key (index)
  :host ((index     :int    :doc "Position in the track list, from 0")
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
         (devices   (list-of device))))

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
         (launch-quantize :string :doc "Scene launch quantization: off, 1 bar, …")))

(def-kind selection
  :key ()
  :host ((track track :set select-track :doc "The current track")))

;; The collections, for (tracks), (scenes) and (banks).
(def-kind project
  :key ()
  :host ((tracks (list-of track))
         (scenes (list-of scene))
         (banks  (list-of bank))))

;; ── Collections and actions ──

(def tracks () project.tracks)
(def scenes () project.scenes)
(def banks () project.banks)

;; Launch a scene with the transport's launch quantization.
(def launch! (s)
  (host-command "switch-pattern"
    (dict :idx s.index :quantize transport.launch-quantize)))

;; Copy scene s to a new scene at the end of its bank, which then plays.
(def clone-scene! (s) (host-command "clone-pattern" (dict :idx s.index)))

;; Delete scene s (never the last one); the playing scene keeps playing
;; unless it is s.
(def delete-scene! (s) (host-command "delete-pattern" (dict :idx s.index)))

;; Load track t's next (dir 1) or previous (dir -1) preset, wrapping.
(def step-preset! (t dir)
  (host-command "step-instrument-preset" (dict :track t.index :delta dir)))
