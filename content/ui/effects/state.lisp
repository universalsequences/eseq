;; Shared view state and sizing constants for the Metal Sequencer effect strip.
;; ui/effects.lisp — Effect chain UI for Metal Sequencer
;; Renders to *fx* buffer. Loaded by ui/main.lisp after shared macro state.
;;
;; The panels' own state is `:key ()` singletons (docs/kind-bindings-spec.md
;; §13.1): one per concern, read by value (`instrument-view.tab`, re-renders
;; the reader) and written with `set!`. Fixtures and Rust tests reach one
;; through a local: `(let ((v eseq.effects.state/effect-mods)) (set! v.open
;; true))`.

(module eseq.effects.state)

(export instrument-view
        key-lock-view
        effect-mods
        process-panel-view
        section-of
        select-section!
        rack-panel-slot-list-open
        rack-panel-selected-chain-open
        rack-panel-macros-open
        rack-panel-set-view
        reset-rack-panel-views!
        seq-timebase-options
        fx-fixed-panel-height
        fx-panel-header-height
        fx-panel-body-content-height)

;; The instrument panel: its tab (0 synth, 1 keys), the sampler's source tab,
;; the mods view and the modulation source (1-4) its knobs edit.
(def-kind instrument-view
  :key ()
  :state ((tab 0)
          (source-tab 0)
          (mods-open false)
          (mod-slot 1)))

;; The keys tab: the octave the piano starts at and how many it shows, the
;; keys selected for key locks, the last plain/cmd-clicked key (shift-click
;; selects the range from it; -1 none: 0 is a valid MIDI note but falsy, so
;; never test it bare) and whether a selected key auditions.
(def-kind key-lock-view
  :key ()
  :state ((octave 3)
          (octave-count 3)
          (anchor -1)
          (notes (list-of :number) :default '())
          (audition true)))

;; The effect whose modulation view is open: its chain (audio, midi, bus,
;; rack), track, slot, rack slot and bus (-1 where it has none), and the
;; modulation source (1-4) its knobs edit.
(def-kind effect-mods
  :key ()
  :state ((open false)
          (chain "audio")
          (track -1)
          (slot -1)
          (rack-slot -1)
          (bus -1)
          (mod-slot 1)))

;; The section selected in each panel scope (a custom UI's, a param grid's),
;; as `(dict :scope :section)` entries, newest first.
(def-kind section-choice
  :key ()
  :state ((sections (list-of :any) :default '())))

;; The section selected in `scope`, else `default`.
(def section-of (scope default)
  (let ((entry (first (filter |item| (= (get item :scope) scope) section-choice.sections))))
    (if entry (get entry :section) default)))

;; Select `section` in `scope` (a no-op when it is selected already, or when
;; `scope` has no entry and `section` is the `default` it displays). The
;; sections are read by value in the *fx* root, so a write re-runs it: a
;; custom UI selects its knob's section on every edit, and storing the
;; default on a scope's first touch re-ran the buffer on that first drag.
(def select-section! (scope section &optional (default nil))
  (unless (= (section-of scope default) section)
    (set! section-choice.sections
      (cons (dict :scope scope :section section)
        (filter |item| (not (= (get item :scope) scope)) section-choice.sections)))))

;; The process panel's selected process: its track and instance id.
(def-kind process-panel-view
  :key ()
  :state ((track -1)
          (instance-id 0)))

;; Each drum rack track's panel layout (its slot list, the selected slot's
;; chain, the macro bank), by the track's stable id (the host's track id, as
;; the string the host passes). Presentation state belongs to stable track
;; identities, never positions; a project replacement forgets every entry
;; (the host calls reset-rack-panel-views!), ordinary scene changes do not.
(def-kind rack-panel-view
  :key ()
  :state ((views (list-of :any) :default '())))

(def rack-view-of (inst)
  (let ((view (first (filter (lambda (view) (= (get view :track-id) (get inst :track-id)))
                       rack-panel-view.views))))
    (or view (dict :slots true :macros false :device true))))

(def rack-panel-slot-list-open (inst) (get (rack-view-of inst) :slots))
(def rack-panel-selected-chain-open (inst) (get (rack-view-of inst) :device))
(def rack-panel-macros-open (inst) (get (rack-view-of inst) :macros))

;; Also called by the host after successfully loading a rack preset or Sound.
(def rack-panel-set-view (track-id slots macros device)
  (set! rack-panel-view.views
    (append
      (filter (lambda (view) (not (= (get view :track-id) track-id))) rack-panel-view.views)
      (list (dict :track-id track-id :slots slots :macros macros :device device)))))

;; Called by the host when a project replaces the previous one.
(def reset-rack-panel-views! ()
  (set! rack-panel-view.views '()))

;; These are temporary render-context globals used by generated custom synth UI.
;; They must NOT be view state: custom UI functions set them while rendering, and
;; writing reactive state during measurement/layout can perturb the layout.
;;
;; They also stay in `eseq.vanilla` explicitly (module spec §3's cross-module
;; def escape hatch) instead of joining this module behind an alias, because
;; they are a host->script protocol, not this module's API: `custom_ui.rs`
;; GENERATES lisp that writes them by bare name (`(set! synth-ui-current-inst
;; inst)`), and that generated unit's compile time is not ordered against this
;; file. An alias would only rescue readers — the stage-3 late-binding heal is
;; read-side — so a generated writer compiled first would keep storing into the
;; stale vanilla slot while later-compiled readers followed the alias to this
;; module's, and the two would silently diverge. Whoever teaches custom_ui.rs
;; to emit qualified names can fold these in.
(def eseq.vanilla/synth-ui-current-inst false)
(def eseq.vanilla/synth-ui-current-name "")
(def eseq.vanilla/midi-fx-ui-current-fx false)
(def eseq.vanilla/midi-fx-ui-current-name "")
(def eseq.vanilla/audio-fx-ui-current-fx false)
(def eseq.vanilla/audio-fx-ui-current-name "")
(def eseq.vanilla/custom-ui-current-kind "instrument")

(def seq-timebase-options
  '("1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh"))

;; Matches a standard built-in FX panel with four parameter rows.
(def fx-fixed-panel-height 10.8)
(def fx-panel-header-height 1.0)
(def fx-panel-body-padding 0.25)
(def fx-panel-body-top-spacer-height 0.16)
(def fx-panel-body-content-height
  (- fx-fixed-panel-height fx-panel-header-height (* 2 fx-panel-body-padding) fx-panel-body-top-spacer-height))
