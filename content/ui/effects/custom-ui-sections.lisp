;; Section selection and base panel primitives for generated custom UIs.
(module eseq.effects.custom-ui-sections)

;; Import cycle with eseq.effects.custom-ui-runtime (it calls our
;; custom-ui-select-section-in-scope; we call its custom-ui-scope-name).
;; load-once (declared_modules) terminates the cycle — the
;; panel-widgets <-> process-panel precedent (spec §10, S3b wave 2).
(import eseq.effects.custom-ui-runtime :as rt)
(import eseq.effects.state :refer (section-of select-section!))

(export custom-ui-set-active-adsr
        custom-ui-adsr-stage-active?
        custom-ui-adsr-stage-active-binding
        custom-ui-selected-section-for-current-scope
        custom-ui-select-section-in-scope
        ui-select-section
        ui-section-select-callback
        ui-panel-bg
        ui-section
        ui-panel)

;; Every alias below is an identity alias: the callers are the unconverted
;; custom-UI family (custom-ui-controls / custom-ui-lego),
;; per-instrument generated ui.lisp files (content/instruments/**),
;; and Rust-embedded lisp (custom_ui.rs codegen calls
;; `custom-ui-selected-section-for-current-scope`; state_values/tests.rs
;; renders `ui-panel` / `ui-section-select-callback`). The section selected
;; in each scope is eseq.effects.state's (section-of, select-section!). Bare callers cannot see qualified names, so
;; the spellings stay put and the alias is the flat->qualified bridge.
;; `ui-select-section` has no in-repo caller but is part of the generated-UI
;; vocabulary (two Rust harnesses stub it), so it keeps its public alias.

;; The ADSR stage a drag holds (an adsr-editor's :active), for the
;; editor's readouts. Every micro-num / adsr-number readout of a custom UI
;; shows the active stage, so a gesture must not re-render them (eseq-eeng:
;; a multi-second first-drag stall on core/triton): a readout binds its
;; editor's stage flags and reads nothing by value, so a drag only repaints
;; the readouts it lights. An editor's flags are a view-local instance keyed
;; by its custom UI's scope name and its section (sections are numbered from
;; -1, the panel's own envelope; spec §3.1, eseq-0l17.73), so two custom UIs
;; on screen sharing a section number keep their own flags. A readout's
;; render creates its editor's instance (the constructor depends only on its
;; key, which moves when the instance is created or dropped), so a drag
;; finds it and re-renders nothing. Equal writes are no-ops, so mid-gesture
;; drag events change nothing. Instances live as long as the session: a
;; scope is a stable name (an instrument's, an effect slot's), and nothing
;; tells this module when one goes away (section-choice keeps its entries
;; the same way).
(def-kind adsr-gesture
  :key (scope section)
  :state ((attack false) (decay false) (sustain false) (release false)))

;; The stage flags of the editor in `section` of the custom UI named
;; `scope-name` (nil, an unnamed harness scope, keys as "").
(def adsr-gesture-of (scope-name section)
  (adsr-gesture (or scope-name "") section))

;; The gesture whose flags the last drag set (cleared when a drag elsewhere
;; starts). Written by the drag handler only; nothing renders from it.
(def held-gesture nil)

;; The stage table: each stage's flag as a binding (a readout's :active) or
;; read now (`binding` false).
(def stage-flag (g stage binding)
  (match stage
    :attack (if binding #'g.attack g.attack)
    :decay (if binding #'g.decay g.decay)
    :sustain (if binding #'g.sustain g.sustain)
    :release (if binding #'g.release g.release)
    _ false))

(def custom-ui-adsr-stage-active-binding (section stage)
  (stage-flag (adsr-gesture-of (rt/custom-ui-scope-name) section) stage true))

(def custom-ui-adsr-stage-active? (section stage)
  (stage-flag (adsr-gesture-of (rt/custom-ui-scope-name) section) stage false))

;; Pinned to eseq.vanilla (spec §3 escape hatch, hazard i):
;; src/ui/custom_ui.rs:425,682 GENERATES lisp that writes this by bare name
;; (`(set! custom-ui-selected-section (custom-ui-selected-section-for-current-scope))`)
;; from implicit-module units. A name Rust writes by bare spelling is not ours
;; to move; no compat alias is minted for it, and in-module reads below use the
;; qualified `eseq.vanilla/` spelling so they hit the same slot the codegen
;; writes (a bare read would intern this module's own slot and freeze — hazard m).
(def eseq.vanilla/custom-ui-selected-section 0)

;; A drag of scope's ADSR editor in `section` holds stage `active` (false: the
;; drag ended).
(def custom-ui-set-active-adsr (scope section active)
  (let ((g (adsr-gesture-of (get scope :name) section)))
    (do
      (when (and held-gesture (not (= held-gesture g)))
        (set-stage-flags! held-gesture false))
      (set-stage-flags! g active)
      (set! held-gesture g))))

(def set-stage-flags! (g active)
  (do
    (set! g.attack (= active :attack))
    (set! g.decay (= active :decay))
    (set! g.sustain (= active :sustain))
    (set! g.release (= active :release))))

(def custom-ui-selected-section-for-current-scope ()
  (section-of (rt/custom-ui-scope-name) 0))

(def set-selected-section-for-scope (scope-name section)
  (select-section! scope-name section))

(def custom-ui-select-section-in-scope (scope section)
  (set-selected-section-for-scope (get scope :name) section))

(def ui-select-section (section)
  (set-selected-section-for-scope (rt/custom-ui-scope-name) section))

(def ui-section-select-callback (section)
  (let ((scope-name (rt/custom-ui-scope-name)))
    (lambda (info)
      (set-selected-section-for-scope scope-name section))))

(def ui-panel-bg (section)
  (if (= section 0)
    :instrument-group-bg
    (if (= eseq.vanilla/custom-ui-selected-section section)
      :instrument-group-selected-bg
      :instrument-group-bg)))

(def row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :dim :bg :transparent)))

(def panel-header (title)
  (box :width :fill :height 0.5 :h-align :start :v-align :center :padding 0.15
    (label title :font-size 7.5 :color :dim :bg :transparent)))

(def ui-section (title body)
  (box :width :fill :height 3.4
       :background-color :instrument-group-bg
       :border-width 1 :corner-radius 12 :padding 0.15
    (v-stack :width :fill :gap 0.2 :align :start
      (panel-header title)
      body)))

(def ui-panel (title section body)
  (box :width :fill :height 3.4
       :background-color (ui-panel-bg section)
       :border-width 1 :corner-radius 12 :padding 0.15
       :on-click (ui-section-select-callback section)
    (v-stack :width :fill :gap 0.2 :align :start
      (panel-header title)
      body)))
