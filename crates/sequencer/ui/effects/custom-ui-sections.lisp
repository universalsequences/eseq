;; Section selection and base panel primitives for generated custom UIs.
(module eseq.effects.custom-ui-sections)

;; Import cycle with eseq.effects.custom-ui-runtime (it calls our
;; custom-ui-select-section-in-scope; we call its custom-ui-scope-name).
;; load-once (declared_modules) terminates the cycle — the
;; panel-widgets <-> process-panel precedent (spec §10, S3b wave 2).
(import eseq.effects.custom-ui-runtime :as rt)

;; Every alias below is an identity alias: the callers are the unconverted
;; custom-UI family (custom-ui-controls / custom-ui-lego),
;; per-instrument generated ui.lisp files (crates/sequencer/instruments/**),
;; and Rust-embedded lisp (custom_ui.rs codegen calls
;; `custom-ui-selected-section-for-current-scope`; state_values/tests.rs
;; eval-reads `custom-ui-selected-sections` and renders `ui-panel` /
;; `ui-section-select-callback`). Bare callers cannot see qualified names, so
;; the spellings stay put and the alias is the flat->qualified bridge.
;; `ui-select-section` has no in-repo caller but is part of the generated-UI
;; vocabulary (two Rust harnesses stub it), so it keeps its public alias.
(module-compat-alias custom-ui-selected-sections custom-ui-selected-sections)
(module-compat-alias custom-ui-set-active-adsr custom-ui-set-active-adsr)
(module-compat-alias custom-ui-adsr-stage-active? custom-ui-adsr-stage-active?)
(module-compat-alias custom-ui-selected-section-for-current-scope custom-ui-selected-section-for-current-scope)
(module-compat-alias custom-ui-select-section-in-scope custom-ui-select-section-in-scope)
(module-compat-alias ui-select-section ui-select-section)
(module-compat-alias ui-section-select-callback ui-section-select-callback)
(module-compat-alias ui-panel-bg ui-panel-bg)
(module-compat-alias ui-section ui-section)
(module-compat-alias ui-panel ui-panel)

(defstate custom-ui-selected-sections '())
(defstate %active-adsr false)

;; Pinned to eseq.vanilla (spec §3 escape hatch, hazard i):
;; src/ui/custom_ui.rs:425,682 GENERATES lisp that writes this by bare name
;; (`(set! custom-ui-selected-section (custom-ui-selected-section-for-current-scope))`)
;; from implicit-module units. A name Rust writes by bare spelling is not ours
;; to move; no compat alias is minted for it, and in-module reads below use the
;; qualified `eseq.vanilla/` spelling so they hit the same slot the codegen
;; writes (a bare read would intern this module's own slot and freeze — hazard m).
(def eseq.vanilla/custom-ui-selected-section 0)

(def custom-ui-set-active-adsr (scope section active)
  (set! %active-adsr
    (if active
      (dict :scope (get scope :name) :section section :stage active)
      false)))

(def custom-ui-adsr-stage-active? (section stage)
  (if %active-adsr
    (and
      (= (get %active-adsr :scope) (rt/custom-ui-scope-name))
      (= (get %active-adsr :section) section)
      (= (get %active-adsr :stage) stage))
    false))

(def %selected-section-for-scope (scope-name)
  (let ((entry
          (nth
            (filter |item| (= (get item :scope) scope-name)
              custom-ui-selected-sections)
            0)))
    (if entry (get entry :section) 0)))

(def custom-ui-selected-section-for-current-scope ()
  (%selected-section-for-scope (rt/custom-ui-scope-name)))

(def %set-selected-section-for-scope (scope-name section)
  (if (= (%selected-section-for-scope scope-name) section)
    false
    (set! custom-ui-selected-sections
      (cons
        (dict :scope scope-name :section section)
        (filter |item| (not (= (get item :scope) scope-name))
          custom-ui-selected-sections)))))

(def custom-ui-select-section-in-scope (scope section)
  (%set-selected-section-for-scope (get scope :name) section))

(def ui-select-section (section)
  (%set-selected-section-for-scope (rt/custom-ui-scope-name) section))

(def ui-section-select-callback (section)
  (let ((scope-name (rt/custom-ui-scope-name)))
    (lambda (info)
      (%set-selected-section-for-scope scope-name section))))

(def ui-panel-bg (section)
  (if (= section 0)
    :instrument-group-bg
    (if (= eseq.vanilla/custom-ui-selected-section section)
      :instrument-group-selected-bg
      :instrument-group-bg)))

(def %row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :dim :bg :transparent)))

(def %panel-header (title)
  (box :width :fill :height 0.5 :h-align :start :v-align :center :padding 0.15
    (label title :font-size 7.5 :color :dim :bg :transparent)))

(def ui-section (title body)
  (box :width :fill :height 3.4
       :background-color :instrument-group-bg
       :border-width 1 :corner-radius 12 :padding 0.15
    (v-stack :width :fill :gap 0.2 :align :start
      (%panel-header title)
      body)))

(def ui-panel (title section body)
  (box :width :fill :height 3.4
       :background-color (ui-panel-bg section)
       :border-width 1 :corner-radius 12 :padding 0.15
       :on-click (ui-section-select-callback section)
    (v-stack :width :fill :gap 0.2 :align :start
      (%panel-header title)
      body)))
