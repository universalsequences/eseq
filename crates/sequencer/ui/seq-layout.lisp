;; Layout specs and apply-layout commands for panels, piano roll, and patcher views.
;; Extracted from ui/main.lisp (module-system spec slice S2). Headerless on
;; purpose: implicit eseq.vanilla until per-file (module …) headers land in S3.

(def seq-step-and-track-panel-layout-spec ()
  (list :cols :gap 1
    0.78 (seq-main-step-tile-layout-spec)
    0.22 (list :rows :gap 1
      0.48 (list :buf "*step*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 28 :max-width 28)
      0.52 (list :buf "*track*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :max-height 7 :min-height 7 :min-width 28 :max-width 28))))

(def seq-main-panel-layout-spec ()
  (if (seq-arrangement-view?)
    (list :buf "*arrangement*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 25)
    (seq-step-and-track-panel-layout-spec)))

(def seq-collapsible-panel-layout-spec (buffer on-collapse min-width max-width min-height max-height)
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

(def seq-samples-panel-layout-spec (min-width max-width min-height max-height)
  (seq-collapsible-panel-layout-spec "*samples*"
    (lambda () (seq-hide-samples-sidebar))
    min-width max-width min-height max-height))

(def seq-mixer-panel-layout-spec (min-width max-width min-height max-height)
  (seq-collapsible-panel-layout-spec "*mixer*"
    (lambda () (seq-hide-mixer-panel))
    min-width max-width min-height max-height))

;; Patch-editor bottom bar variant of the mixer panel: a single compact
;; channel strip for the current track (see patch-mixer-strip in mixer.lisp).
(def seq-patch-mixer-panel-layout-spec (min-width max-width min-height max-height)
  (seq-collapsible-panel-layout-spec "*patch-mixer*"
    (lambda () (seq-hide-mixer-panel))
    min-width max-width min-height max-height))

(def seq-fx-panel-layout-spec (min-width max-width min-height max-height)
  (seq-collapsible-panel-layout-spec "*fx*"
    (lambda () (seq-hide-fx-panel))
    min-width max-width min-height max-height))

(def seq-samples-sidebar-layout-spec ()
  (if (param-macro-mapping-active?)
    (seq-collapsible-panel-layout-spec "*macro-mappings*"
      (lambda () (macro-clear-mapping-arm))
      46 64 nil nil)
    (seq-samples-panel-layout-spec 34 42 nil nil)))

(def seq-sidebar-ratio ()
  (if (param-macro-mapping-active?) 0.34 0.2))

(def seq-main-and-mixer-layout-spec ()
  (if mixer-panel-visible
    (list :rows :gap 1
      0.55 (seq-main-panel-layout-spec)
      0.45 (seq-mixer-panel-layout-spec nil nil 14.5 14.5))
    (seq-main-panel-layout-spec)))

(def seq-lower-panel-layout-spec (lower-buffer lower-ratio lower-min-height lower-max-height)
  (let ((main-layout
          (if samples-sidebar-visible
            (list :cols :gap 1
              (seq-sidebar-ratio) (seq-samples-sidebar-layout-spec)
              (- 1.0 (seq-sidebar-ratio)) (seq-main-and-mixer-layout-spec))
            (seq-main-and-mixer-layout-spec))))
    (if lower-panel-visible
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.95 (list :rows :gap 1 :remember (str "sequencer-lower-panel:" lower-buffer)
          0.95 main-layout
          lower-ratio (if (= lower-buffer "*fx*")
            (seq-fx-panel-layout-spec nil nil lower-min-height lower-max-height)
            (list :buf lower-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height lower-min-height :max-height lower-max-height))))
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.95 main-layout))))

;; Every patcher bottom-bar panel shares the regular fx-panel height so the
;; instrument preview isn't padded out by a taller neighbor.
(def seq-patcher-bottom-bar-panel-height lower-fx-layout-height)

(def seq-patcher-bottom-bar-layout-spec ()
  (let ((h seq-patcher-bottom-bar-panel-height))
    (if (and samples-sidebar-visible mixer-panel-visible lower-panel-visible)
      (list :cols :gap 1
        0.30 (seq-samples-panel-layout-spec 28 28 h h)
        0.15 (seq-patch-mixer-panel-layout-spec 10 10 h h)
        0.55 (seq-fx-panel-layout-spec nil nil h h))
      (if (and samples-sidebar-visible mixer-panel-visible)
        (list :cols :gap 1
          0.6 (seq-samples-panel-layout-spec 28 28 h h)
          0.4 (seq-patch-mixer-panel-layout-spec 10 10 h h))
      (if samples-sidebar-visible
        (if lower-panel-visible
          (list :cols :gap 1
            0.5 (seq-samples-panel-layout-spec 28 28 h h)
            0.5 (seq-fx-panel-layout-spec nil nil h h))
          (seq-samples-panel-layout-spec 28 28 h h))
        (if mixer-panel-visible
          (if lower-panel-visible
            (list :cols :gap 1
              0.4 (seq-patch-mixer-panel-layout-spec 10 10 h h)
              0.6 (seq-fx-panel-layout-spec nil nil h h))
            (seq-patch-mixer-panel-layout-spec 15 15 h h))
          (seq-fx-panel-layout-spec nil nil h h)))))))

(def seq-patcher-bottom-bar-visible? ()
  (or samples-sidebar-visible mixer-panel-visible lower-panel-visible))

;; Macro sidebar to the left of the patch editor: defmacros in the patch +
;; the saved macro library (see ui/patch-macros.lisp).
(def seq-patch-macros-panel-layout-spec ()
  (seq-collapsible-panel-layout-spec "*patch-macros*"
    (lambda () (seq-hide-patch-macros-panel))
    22 26 nil nil))

(def seq-patcher-canvas-layout-spec (patcher-buffer)
  (list :buf patcher-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height 20))

(def seq-patcher-main-layout-spec (patcher-buffer)
  (if patch-macros-panel-visible
    (list :cols :gap 1
      0.15 (seq-patch-macros-panel-layout-spec)
      0.85 (seq-patcher-canvas-layout-spec patcher-buffer))
    (seq-patcher-canvas-layout-spec patcher-buffer)))

(def seq-instrument-patcher-layout-spec (patcher-buffer)
  (if (seq-patcher-bottom-bar-visible?)
    (list :rows :gap 1
      0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
      0.80 (seq-patcher-main-layout-spec patcher-buffer)
      0.15 (seq-patcher-bottom-bar-layout-spec))
    (list :rows :gap 1
      0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
      0.95 (seq-patcher-main-layout-spec patcher-buffer))))

(def seq-instrument-patcher-source-layout-spec (patcher-buffer source-buffer)
  (let ((main-layout
          (if patch-macros-panel-visible
            (list :cols :gap 1
              0.14 (seq-patch-macros-panel-layout-spec)
              0.53 (seq-patcher-canvas-layout-spec patcher-buffer)
              0.33 (list :buf source-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height 20))
            (list :cols :gap 1
              0.62 (seq-patcher-canvas-layout-spec patcher-buffer)
              0.38 (list :buf source-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height 20)))))
    (if (seq-patcher-bottom-bar-visible?)
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.80 main-layout
        0.15 (seq-patcher-bottom-bar-layout-spec))
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.95 main-layout))))

(def seq-apply-lower-panel-layout (lower-buffer lower-ratio lower-min-height lower-max-height)
  (do
    (set! seq-layout-mode :lower-panel)
    (set-layout (seq-lower-panel-layout-spec lower-buffer lower-ratio lower-min-height lower-max-height))
    (host-command "refresh-mixer-ui" (dict))))

(def seq-apply-fx-layout ()
  (do
    (set! lower-panel-buffer "*fx*")
    (seq-apply-lower-panel-layout "*fx*" 0.33 lower-fx-layout-height lower-fx-layout-height)))

(def seq-apply-piano-roll-layout ()
  (do
    (set! lower-panel-buffer "*piano-roll*")
    (seq-apply-lower-panel-layout "*piano-roll*" 0.33 lower-fx-layout-height 50)))

(def seq-apply-instrument-patcher-layout (patcher-buffer)
  (do
    (set! remembered-step-panel-buffer (seq-current-step-buffer))
    (set! seq-layout-mode :instrument-patcher)
    (set! seq-patcher-buffer patcher-buffer)
    (set! seq-patcher-source-buffer "")
    (set-layout (seq-instrument-patcher-layout-spec patcher-buffer))
    (host-command "refresh-mixer-ui" (dict))))

(def seq-apply-instrument-patcher-source-layout (patcher-buffer source-buffer)
  (do
    (set! remembered-step-panel-buffer (seq-current-step-buffer))
    (set! seq-layout-mode :instrument-patcher-source)
    (set! seq-patcher-buffer patcher-buffer)
    (set! seq-patcher-source-buffer source-buffer)
    (set-layout (seq-instrument-patcher-source-layout-spec patcher-buffer source-buffer))
    (host-command "refresh-mixer-ui" (dict))))

(def seq-refresh-current-layout ()
  (if (and (= seq-layout-mode :instrument-patcher-source) (not (= seq-patcher-buffer "")) (not (= seq-patcher-source-buffer "")))
    (seq-apply-instrument-patcher-source-layout seq-patcher-buffer seq-patcher-source-buffer)
    (if (and (= seq-layout-mode :instrument-patcher) (not (= seq-patcher-buffer "")))
      (seq-apply-instrument-patcher-layout seq-patcher-buffer)
      (if (= lower-panel-buffer "*piano-roll*")
        (seq-apply-piano-roll-layout)
        (seq-apply-fx-layout)))))
