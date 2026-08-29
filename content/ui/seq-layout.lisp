;; Layout specs and apply-layout commands for panels, piano roll, and patcher views.
;; Extracted from ui/main.lisp (module-system spec slice S2), converted to a
;; module in S3.

(module eseq.seq-layout)
;; Compile-time edges (spec §4): the shared defstate keyspace + compat
;; aliases must exist before this unit's readers compile, and this file
;; reads eseq.seq-step-tabs at LOAD time (`lower-fx-layout-height` feeds a
;; top-level def) as well as its layout defstates.
(import eseq.seq-core-state)
(import eseq.seq-step-tabs)

(import eseq.effects.param-controls :as pc)

(export buffer-radius
        step-and-track-panel-layout-spec
        main-panel-layout-spec
        lower-panel-layout-spec
        instrument-patcher-layout-spec
        instrument-patcher-source-layout-spec
        instrument-patcher-learn-layout-spec
        apply-lower-panel-layout
        apply-fx-layout
        apply-piano-roll-layout
        apply-instrument-patcher-layout
        apply-instrument-patcher-source-layout
        apply-instrument-patcher-learn-layout
        refresh-current-layout)

;; Migration aliases (module spec §10): the names an unconverted caller — or
;; Rust — still spells flat. `apply-fx-layout` is the production startup
;; expression (`STARTUP_GRID_LAYOUT_EXPR`, src/ui/editor_setup.rs) and the two
;; patcher applies are `format!`-built evals in src/ui/edit_sessions.rs;
;; `refresh-current-layout` is called from seq-panels.lisp, seq-step-tabs.lisp
;; and from the already-converted eseq.seq-macro-mapping-hooks module (whose
;; bare call resolves through this table, since seq-layout.lisp loads first);
;; the two `*-layout-spec` entries are evaled by name from Rust tests. The
;; `apply-*` names are also the M-x-visible spellings.

(def buffer-radius 16)

(def step-and-track-panel-layout-spec ()
  (list :cols :gap 1
    0.78 (eseq.seq-step-tabs/seq-main-step-tile-layout-spec)
    0.22 (list :rows :gap 1
      0.48 (list :buf "*step*" :hide-status true :border-radius buffer-radius :border-width 4 :background-color :buffer-bg :min-width 28 :max-width 28)
      0.52 (list :buf "*track*" :hide-status true :border-radius buffer-radius :border-width 4 :background-color :buffer-bg :max-height 7 :min-height 7 :min-width 28 :max-width 28))))

(def main-panel-layout-spec ()
  (if (eseq.seq-step-tabs/seq-arrangement-view?)
    (list :buf "*arrangement*" :hide-status true :border-radius buffer-radius :border-width 4 :background-color :buffer-bg :min-width 25)
    (step-and-track-panel-layout-spec)))

(def collapsible-panel-layout-spec (buffer on-collapse min-width max-width min-height max-height)
  (list :buf buffer
    :hide-status true
    :border-radius buffer-radius
    :border-width 4
    :background-color :buffer-bg
    :min-width min-width
    :max-width max-width
    :min-height min-height
    :max-height max-height
    :collapse-threshold 0.25
    :on-collapse on-collapse))

(def samples-panel-layout-spec (min-width max-width min-height max-height)
  (collapsible-panel-layout-spec "*samples*"
    (lambda () (eseq.seq-panels/seq-hide-samples-sidebar))
    min-width max-width min-height max-height))

(def mixer-panel-layout-spec (min-width max-width min-height max-height)
  (collapsible-panel-layout-spec "*mixer*"
    (lambda () (eseq.seq-panels/seq-hide-mixer-panel))
    min-width max-width min-height max-height))

;; Patch-editor bottom bar variant of the mixer panel: a single compact
;; channel strip for the current track (see patch-mixer-strip in mixer.lisp).
(def patch-mixer-panel-layout-spec (min-width max-width min-height max-height)
  (collapsible-panel-layout-spec "*patch-mixer*"
    (lambda () (eseq.seq-panels/seq-hide-mixer-panel))
    min-width max-width min-height max-height))

(def fx-panel-layout-spec (min-width max-width min-height max-height)
  (collapsible-panel-layout-spec "*fx*"
    (lambda () (eseq.seq-panels/seq-hide-fx-panel))
    min-width max-width min-height max-height))

(def samples-sidebar-layout-spec ()
  (if (pc/param-macro-mapping-active?)
    (collapsible-panel-layout-spec "*macro-mappings*"
      (lambda () (eseq.macro-state/clear-mapping-arm))
      46 64 nil nil)
    (samples-panel-layout-spec 34 42 nil nil)))

(def sidebar-ratio ()
  (if (pc/param-macro-mapping-active?) 0.34 0.2))

(def main-and-mixer-layout-spec ()
  (if eseq.seq-core-state/mixer-panel-visible
    (list :rows :gap 1
      0.55 (main-panel-layout-spec)
      0.45 (mixer-panel-layout-spec nil nil 14.5 14.5))
    (main-panel-layout-spec)))

(def lower-panel-layout-spec (lower-buffer lower-ratio lower-min-height lower-max-height)
  (let ((main-layout
          (if eseq.seq-core-state/samples-sidebar-visible
            (list :cols :gap 1
              (sidebar-ratio) (samples-sidebar-layout-spec)
              (- 1.0 (sidebar-ratio)) (main-and-mixer-layout-spec))
            (main-and-mixer-layout-spec))))
    (if eseq.seq-core-state/lower-panel-visible
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.95 (list :rows :gap 1 :remember (str "sequencer-lower-panel:" lower-buffer)
          0.95 main-layout
          lower-ratio (if (= lower-buffer "*fx*")
            (fx-panel-layout-spec nil nil lower-min-height lower-max-height)
            (list :buf lower-buffer :hide-status true :border-radius buffer-radius :border-width 4 :background-color :buffer-bg :min-height lower-min-height :max-height lower-max-height))))
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.95 main-layout))))

;; Every patcher bottom-bar panel shares the regular fx-panel height so the
;; instrument preview isn't padded out by a taller neighbor.
(def patcher-bottom-bar-panel-height eseq.seq-step-tabs/lower-fx-layout-height)

(def patcher-bottom-bar-layout-spec ()
  (let ((h patcher-bottom-bar-panel-height))
    (if (and eseq.seq-core-state/samples-sidebar-visible eseq.seq-core-state/mixer-panel-visible eseq.seq-core-state/lower-panel-visible)
      (list :cols :gap 1
        0.30 (samples-panel-layout-spec 28 28 h h)
        0.15 (patch-mixer-panel-layout-spec 10 10 h h)
        0.55 (fx-panel-layout-spec nil nil h h))
      (if (and eseq.seq-core-state/samples-sidebar-visible eseq.seq-core-state/mixer-panel-visible)
        (list :cols :gap 1
          0.6 (samples-panel-layout-spec 28 28 h h)
          0.4 (patch-mixer-panel-layout-spec 10 10 h h))
      (if eseq.seq-core-state/samples-sidebar-visible
        (if eseq.seq-core-state/lower-panel-visible
          (list :cols :gap 1
            0.5 (samples-panel-layout-spec 28 28 h h)
            0.5 (fx-panel-layout-spec nil nil h h))
          (samples-panel-layout-spec 28 28 h h))
        (if eseq.seq-core-state/mixer-panel-visible
          (if eseq.seq-core-state/lower-panel-visible
            (list :cols :gap 1
              0.4 (patch-mixer-panel-layout-spec 10 10 h h)
              0.6 (fx-panel-layout-spec nil nil h h))
            (patch-mixer-panel-layout-spec 15 15 h h))
          (fx-panel-layout-spec nil nil h h)))))))

(def patcher-bottom-bar-visible? ()
  (or eseq.seq-core-state/samples-sidebar-visible eseq.seq-core-state/mixer-panel-visible eseq.seq-core-state/lower-panel-visible))

;; Macro sidebar to the left of the patch editor: defmacros in the patch +
;; the saved macro library (see ui/patch-macros.lisp).
(def patch-macros-panel-layout-spec ()
  (collapsible-panel-layout-spec "*patch-macros*"
    (lambda () (eseq.seq-panels/seq-hide-patch-macros-panel))
    22 26 nil nil))

(def patcher-canvas-layout-spec (patcher-buffer)
  (list :buf patcher-buffer :hide-status true :border-radius buffer-radius :border-width 4 :background-color :buffer-bg :min-height 20))

(def patch-learn-buffer-layout-spec (learn-buffer)
  (list :buf learn-buffer :hide-status true :border-radius buffer-radius :border-width 4 :background-color :buffer-bg :min-width 28 :min-height 20))

(def patcher-main-layout-spec (patcher-buffer)
  (if eseq.seq-core-state/patch-macros-panel-visible
    (list :cols :gap 1
      0.15 (patch-macros-panel-layout-spec)
      0.85 (patcher-canvas-layout-spec patcher-buffer))
    (patcher-canvas-layout-spec patcher-buffer)))

(def instrument-patcher-layout-spec (patcher-buffer)
  (if (patcher-bottom-bar-visible?)
    (list :rows :gap 1
      0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
      0.80 (patcher-main-layout-spec patcher-buffer)
      0.15 (patcher-bottom-bar-layout-spec))
    (list :rows :gap 1
      0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
      0.95 (patcher-main-layout-spec patcher-buffer))))

(def instrument-patcher-source-layout-spec (patcher-buffer source-buffer)
  (let ((main-layout
          (if eseq.seq-core-state/patch-macros-panel-visible
            (list :cols :gap 1
              0.14 (patch-macros-panel-layout-spec)
              0.53 (patcher-canvas-layout-spec patcher-buffer)
              0.33 (list :buf source-buffer :hide-status true :border-raduis buffer-radius :border-width 4 :background-color :buffer-bg :min-height 20))
            (list :cols :gap 1
              0.62 (patcher-canvas-layout-spec patcher-buffer)
              0.38 (list :buf source-buffer :hide-status true :border-radius buffer-radius :border-width 4 :background-color :buffer-bg :min-height 20)))))
    (if (patcher-bottom-bar-visible?)
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.80 main-layout
        0.15 (patcher-bottom-bar-layout-spec))
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.95 main-layout))))

;; Patch Learn is a real sibling buffer in the editor's tile tree. Keeping it
;; outside the patcher render root prevents either pane from clipping, sizing,
;; or routing pointer input through the other one.
(def instrument-patcher-learn-layout-spec (patcher-buffer learn-buffer)
  (let ((patcher-and-learn
          (list :cols :gap 1 :remember (str "instrument-patcher-learn:" patcher-buffer)
            0.68 (patcher-canvas-layout-spec patcher-buffer)
            0.32 (patch-learn-buffer-layout-spec learn-buffer)))
        (main-layout
          (if eseq.seq-core-state/patch-macros-panel-visible
            (list :cols :gap 1
              0.15 (patch-macros-panel-layout-spec)
              0.85 patcher-and-learn)
            patcher-and-learn)))
    (if (patcher-bottom-bar-visible?)
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.80 main-layout
        0.15 (patcher-bottom-bar-layout-spec))
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.95 main-layout))))

(def apply-lower-panel-layout (lower-buffer lower-ratio lower-min-height lower-max-height)
  (do
    (set! eseq.seq-step-tabs/seq-layout-mode :lower-panel)
    (set-layout (lower-panel-layout-spec lower-buffer lower-ratio lower-min-height lower-max-height))
    (host-command "refresh-mixer-ui" (dict))))

(def apply-fx-layout ()
  (do
    (set! eseq.seq-step-tabs/lower-panel-buffer "*fx*")
    (apply-lower-panel-layout "*fx*" 0.33 eseq.seq-step-tabs/lower-fx-layout-height eseq.seq-step-tabs/lower-fx-layout-height)))

(def apply-piano-roll-layout ()
  (do
    (set! eseq.seq-step-tabs/lower-panel-buffer "*piano-roll*")
    (apply-lower-panel-layout "*piano-roll*" 0.33 eseq.seq-step-tabs/lower-fx-layout-height 50)))

(def apply-instrument-patcher-layout (patcher-buffer)
  (do
    (set! eseq.seq-step-tabs/remembered-step-panel-buffer (eseq.seq-panels/seq-current-step-buffer))
    (set! eseq.seq-step-tabs/seq-layout-mode :instrument-patcher)
    (set! eseq.seq-step-tabs/seq-patcher-buffer patcher-buffer)
    (set! eseq.seq-step-tabs/seq-patcher-source-buffer "")
    (set! eseq.seq-step-tabs/seq-patcher-learn-buffer "")
    (set-layout (instrument-patcher-layout-spec patcher-buffer))
    (host-command "refresh-mixer-ui" (dict))))

(def apply-instrument-patcher-source-layout (patcher-buffer source-buffer)
  (do
    (set! eseq.seq-step-tabs/remembered-step-panel-buffer (eseq.seq-panels/seq-current-step-buffer))
    (set! eseq.seq-step-tabs/seq-layout-mode :instrument-patcher-source)
    (set! eseq.seq-step-tabs/seq-patcher-buffer patcher-buffer)
    (set! eseq.seq-step-tabs/seq-patcher-source-buffer source-buffer)
    (set! eseq.seq-step-tabs/seq-patcher-learn-buffer "")
    (set-layout (instrument-patcher-source-layout-spec patcher-buffer source-buffer))
    (host-command "refresh-mixer-ui" (dict))))

(def apply-instrument-patcher-learn-layout (patcher-buffer learn-buffer)
  (do
    (set! eseq.seq-step-tabs/remembered-step-panel-buffer (eseq.seq-panels/seq-current-step-buffer))
    (set! eseq.seq-step-tabs/seq-layout-mode :instrument-patcher-learn)
    (set! eseq.seq-step-tabs/seq-patcher-buffer patcher-buffer)
    (set! eseq.seq-step-tabs/seq-patcher-source-buffer "")
    (set! eseq.seq-step-tabs/seq-patcher-learn-buffer learn-buffer)
    (set-layout (instrument-patcher-learn-layout-spec patcher-buffer learn-buffer))
    (host-command "refresh-mixer-ui" (dict))))

(def refresh-current-layout ()
  (if (and (= eseq.seq-step-tabs/seq-layout-mode :instrument-patcher-learn) (not (= eseq.seq-step-tabs/seq-patcher-buffer "")) (not (= eseq.seq-step-tabs/seq-patcher-learn-buffer "")))
    (apply-instrument-patcher-learn-layout eseq.seq-step-tabs/seq-patcher-buffer eseq.seq-step-tabs/seq-patcher-learn-buffer)
    (if (and (= eseq.seq-step-tabs/seq-layout-mode :instrument-patcher-source) (not (= eseq.seq-step-tabs/seq-patcher-buffer "")) (not (= eseq.seq-step-tabs/seq-patcher-source-buffer "")))
      (apply-instrument-patcher-source-layout eseq.seq-step-tabs/seq-patcher-buffer eseq.seq-step-tabs/seq-patcher-source-buffer)
      (if (and (= eseq.seq-step-tabs/seq-layout-mode :instrument-patcher) (not (= eseq.seq-step-tabs/seq-patcher-buffer "")))
        (apply-instrument-patcher-layout eseq.seq-step-tabs/seq-patcher-buffer)
        (if (= eseq.seq-step-tabs/lower-panel-buffer "*piano-roll*")
          (apply-piano-roll-layout)
          (apply-fx-layout))))))
