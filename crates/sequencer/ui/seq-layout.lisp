;; Layout specs and apply-layout commands for panels, piano roll, and patcher views.
;; Extracted from ui/main.lisp (module-system spec slice S2), converted to a
;; module in S3.

(module eseq.seq-layout)

;; Migration aliases (module spec §10): the names an unconverted caller — or
;; Rust — still spells flat. `apply-fx-layout` is the production startup
;; expression (`STARTUP_GRID_LAYOUT_EXPR`, src/ui/editor_setup.rs) and the two
;; patcher applies are `format!`-built evals in src/ui/edit_sessions.rs;
;; `refresh-current-layout` is called from seq-panels.lisp, seq-step-tabs.lisp
;; and from the already-converted eseq.seq-macro-mapping-hooks module (whose
;; bare call resolves through this table, since seq-layout.lisp loads first);
;; the two `*-layout-spec` entries are evaled by name from Rust tests. The
;; `apply-*` names are also the M-x-visible spellings.
(module-compat-alias seq-apply-fx-layout apply-fx-layout)
(module-compat-alias seq-apply-piano-roll-layout apply-piano-roll-layout)
(module-compat-alias seq-apply-lower-panel-layout apply-lower-panel-layout)
(module-compat-alias seq-apply-instrument-patcher-layout apply-instrument-patcher-layout)
(module-compat-alias seq-apply-instrument-patcher-source-layout apply-instrument-patcher-source-layout)
(module-compat-alias seq-refresh-current-layout refresh-current-layout)
(module-compat-alias seq-lower-panel-layout-spec lower-panel-layout-spec)
(module-compat-alias seq-step-and-track-panel-layout-spec step-and-track-panel-layout-spec)

(def step-and-track-panel-layout-spec ()
  (list :cols :gap 1
    0.78 (seq-main-step-tile-layout-spec)
    0.22 (list :rows :gap 1
      0.48 (list :buf "*step*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 28 :max-width 28)
      0.52 (list :buf "*track*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :max-height 7 :min-height 7 :min-width 28 :max-width 28))))

(def main-panel-layout-spec ()
  (if (seq-arrangement-view?)
    (list :buf "*arrangement*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 25)
    (step-and-track-panel-layout-spec)))

(def %collapsible-panel-layout-spec (buffer on-collapse min-width max-width min-height max-height)
  (list :buf buffer
    :hide-status true
    :border-radius 12
    :border-width 4
    :background-color :buffer-bg
    :min-width min-width
    :max-width max-width
    :min-height min-height
    :max-height max-height
    :collapse-threshold 0.25
    :on-collapse on-collapse))

(def %samples-panel-layout-spec (min-width max-width min-height max-height)
  (%collapsible-panel-layout-spec "*samples*"
    (lambda () (seq-hide-samples-sidebar))
    min-width max-width min-height max-height))

(def %mixer-panel-layout-spec (min-width max-width min-height max-height)
  (%collapsible-panel-layout-spec "*mixer*"
    (lambda () (seq-hide-mixer-panel))
    min-width max-width min-height max-height))

;; Patch-editor bottom bar variant of the mixer panel: a single compact
;; channel strip for the current track (see patch-mixer-strip in mixer.lisp).
(def %patch-mixer-panel-layout-spec (min-width max-width min-height max-height)
  (%collapsible-panel-layout-spec "*patch-mixer*"
    (lambda () (seq-hide-mixer-panel))
    min-width max-width min-height max-height))

(def %fx-panel-layout-spec (min-width max-width min-height max-height)
  (%collapsible-panel-layout-spec "*fx*"
    (lambda () (seq-hide-fx-panel))
    min-width max-width min-height max-height))

(def %samples-sidebar-layout-spec ()
  (if (param-macro-mapping-active?)
    (%collapsible-panel-layout-spec "*macro-mappings*"
      (lambda () (macro-clear-mapping-arm))
      46 64 nil nil)
    (%samples-panel-layout-spec 34 42 nil nil)))

(def %sidebar-ratio ()
  (if (param-macro-mapping-active?) 0.34 0.2))

(def %main-and-mixer-layout-spec ()
  (if mixer-panel-visible
    (list :rows :gap 1
      0.55 (main-panel-layout-spec)
      0.45 (%mixer-panel-layout-spec nil nil 14.5 14.5))
    (main-panel-layout-spec)))

(def lower-panel-layout-spec (lower-buffer lower-ratio lower-min-height lower-max-height)
  (let ((main-layout
          (if samples-sidebar-visible
            (list :cols :gap 1
              (%sidebar-ratio) (%samples-sidebar-layout-spec)
              (- 1.0 (%sidebar-ratio)) (%main-and-mixer-layout-spec))
            (%main-and-mixer-layout-spec))))
    (if lower-panel-visible
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.95 (list :rows :gap 1 :remember (str "sequencer-lower-panel:" lower-buffer)
          0.95 main-layout
          lower-ratio (if (= lower-buffer "*fx*")
            (%fx-panel-layout-spec nil nil lower-min-height lower-max-height)
            (list :buf lower-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height lower-min-height :max-height lower-max-height))))
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.95 main-layout))))

;; Every patcher bottom-bar panel shares the regular fx-panel height so the
;; instrument preview isn't padded out by a taller neighbor.
(def %patcher-bottom-bar-panel-height lower-fx-layout-height)

(def %patcher-bottom-bar-layout-spec ()
  (let ((h %patcher-bottom-bar-panel-height))
    (if (and samples-sidebar-visible mixer-panel-visible lower-panel-visible)
      (list :cols :gap 1
        0.30 (%samples-panel-layout-spec 28 28 h h)
        0.15 (%patch-mixer-panel-layout-spec 10 10 h h)
        0.55 (%fx-panel-layout-spec nil nil h h))
      (if (and samples-sidebar-visible mixer-panel-visible)
        (list :cols :gap 1
          0.6 (%samples-panel-layout-spec 28 28 h h)
          0.4 (%patch-mixer-panel-layout-spec 10 10 h h))
      (if samples-sidebar-visible
        (if lower-panel-visible
          (list :cols :gap 1
            0.5 (%samples-panel-layout-spec 28 28 h h)
            0.5 (%fx-panel-layout-spec nil nil h h))
          (%samples-panel-layout-spec 28 28 h h))
        (if mixer-panel-visible
          (if lower-panel-visible
            (list :cols :gap 1
              0.4 (%patch-mixer-panel-layout-spec 10 10 h h)
              0.6 (%fx-panel-layout-spec nil nil h h))
            (%patch-mixer-panel-layout-spec 15 15 h h))
          (%fx-panel-layout-spec nil nil h h)))))))

(def %patcher-bottom-bar-visible? ()
  (or samples-sidebar-visible mixer-panel-visible lower-panel-visible))

;; Macro sidebar to the left of the patch editor: defmacros in the patch +
;; the saved macro library (see ui/patch-macros.lisp).
(def %patch-macros-panel-layout-spec ()
  (%collapsible-panel-layout-spec "*patch-macros*"
    (lambda () (seq-hide-patch-macros-panel))
    22 26 nil nil))

(def %patcher-canvas-layout-spec (patcher-buffer)
  (list :buf patcher-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height 20))

(def %patcher-main-layout-spec (patcher-buffer)
  (if patch-macros-panel-visible
    (list :cols :gap 1
      0.15 (%patch-macros-panel-layout-spec)
      0.85 (%patcher-canvas-layout-spec patcher-buffer))
    (%patcher-canvas-layout-spec patcher-buffer)))

(def instrument-patcher-layout-spec (patcher-buffer)
  (if (%patcher-bottom-bar-visible?)
    (list :rows :gap 1
      0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
      0.80 (%patcher-main-layout-spec patcher-buffer)
      0.15 (%patcher-bottom-bar-layout-spec))
    (list :rows :gap 1
      0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
      0.95 (%patcher-main-layout-spec patcher-buffer))))

(def instrument-patcher-source-layout-spec (patcher-buffer source-buffer)
  (let ((main-layout
          (if patch-macros-panel-visible
            (list :cols :gap 1
              0.14 (%patch-macros-panel-layout-spec)
              0.53 (%patcher-canvas-layout-spec patcher-buffer)
              0.33 (list :buf source-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height 20))
            (list :cols :gap 1
              0.62 (%patcher-canvas-layout-spec patcher-buffer)
              0.38 (list :buf source-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height 20)))))
    (if (%patcher-bottom-bar-visible?)
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.80 main-layout
        0.15 (%patcher-bottom-bar-layout-spec))
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.95 main-layout))))

(def apply-lower-panel-layout (lower-buffer lower-ratio lower-min-height lower-max-height)
  (do
    (set! seq-layout-mode :lower-panel)
    (set-layout (lower-panel-layout-spec lower-buffer lower-ratio lower-min-height lower-max-height))
    (host-command "refresh-mixer-ui" (dict))))

(def apply-fx-layout ()
  (do
    (set! lower-panel-buffer "*fx*")
    (apply-lower-panel-layout "*fx*" 0.33 lower-fx-layout-height lower-fx-layout-height)))

(def apply-piano-roll-layout ()
  (do
    (set! lower-panel-buffer "*piano-roll*")
    (apply-lower-panel-layout "*piano-roll*" 0.33 lower-fx-layout-height 50)))

(def apply-instrument-patcher-layout (patcher-buffer)
  (do
    (set! remembered-step-panel-buffer (seq-current-step-buffer))
    (set! seq-layout-mode :instrument-patcher)
    (set! seq-patcher-buffer patcher-buffer)
    (set! seq-patcher-source-buffer "")
    (set-layout (instrument-patcher-layout-spec patcher-buffer))
    (host-command "refresh-mixer-ui" (dict))))

(def apply-instrument-patcher-source-layout (patcher-buffer source-buffer)
  (do
    (set! remembered-step-panel-buffer (seq-current-step-buffer))
    (set! seq-layout-mode :instrument-patcher-source)
    (set! seq-patcher-buffer patcher-buffer)
    (set! seq-patcher-source-buffer source-buffer)
    (set-layout (instrument-patcher-source-layout-spec patcher-buffer source-buffer))
    (host-command "refresh-mixer-ui" (dict))))

(def refresh-current-layout ()
  (if (and (= seq-layout-mode :instrument-patcher-source) (not (= seq-patcher-buffer "")) (not (= seq-patcher-source-buffer "")))
    (apply-instrument-patcher-source-layout seq-patcher-buffer seq-patcher-source-buffer)
    (if (and (= seq-layout-mode :instrument-patcher) (not (= seq-patcher-buffer "")))
      (apply-instrument-patcher-layout seq-patcher-buffer)
      (if (= lower-panel-buffer "*piano-roll*")
        (apply-piano-roll-layout)
        (apply-fx-layout)))))
