;; ui/transport.lisp — Transport bar UI (Logic Pro style)
;; Renders to *transport* buffer. Loaded by ui/main.lisp. Converted in S3b.
;;
;; NEVER `(import eseq.transport …)`. `import` EVALUATES its target and this
;; file is a live render root: the `(effect-buffer "*transport*" …)` at the
;; bottom registers a buffer at top level, so importing it would drag the
;; transport UI into every VM that loads the importer (the wave-2 lesson that
;; broke 60 tests). Callers reach the names below through the identity compat
;; aliases instead — which is also why `pattern-control-style`, used by the
;; converted `eseq.sequencer`, is aliased rather than requalified at its call
;; sites.
;;
;; Every other outbound reference evaluated at event time is left bare: a
;; Rust native (`host-command`), a function in ui/seq-panels.lisp (the panel
;; toggles and the two view switches), or a name owned by a UI ROOT module
;; reached through that root's own compat alias (`set-arrangement-cursor` →
;; eseq.arrangement, `sbrowser-*` → eseq.browser). All of those are reached
;; only from `on-click` lambdas — i.e. at event time, long after main.lisp has
;; loaded everything. A reference evaluated during the RENDER cannot rely on
;; that; see the eseq.seq-step-tabs import note below.
;;
;; Host state comes from eseq.kinds (docs/kind-bindings-spec.md): `transport`,
;; `master`, `engine`, `song`, the banks and their scenes. A field read by
;; value (`transport.sequence-rolling`) re-renders its subtree; a `#'` binding
;; (`#'transport.playing` on an icon, `#'engine.cpu-load` on the readout)
;; only repaints. The menus, the bank rename and the scene push are this
;; view's own state: `:key ()` singletons below.
;;
;; The three panel-visibility reads (`samples-sidebar-visible`,
;; `mixer-panel-visible`, `lower-panel-visible`) are qualified reads of
;; `defstate`s owned by `eseq.seq-core-state`, which main.lisp loads first.
(module eseq.transport)
;; Compile-time edge (spec §4): the shared defstate keyspace + compat
;; aliases must exist before this unit's readers compile.
(import eseq.seq-core-state)

;; This import is load-bearing rather than cosmetic. The two view
;; buttons at the right edge call `seq-arrangement-view?` at RENDER time, and
;; main.lisp loads ui/seq-step-tabs.lisp (its owner) at line 41 — sixteen lines
;; AFTER this file. The transport's effect body runs once at load, so that call
;; hits an empty slot; a headerless file survives that because the flat slot it
;; interned is later filled by the vanilla `def`, but a module's
;; `eseq.transport/seq-arrangement-view?` slot has nothing to heal onto at that
;; instant and the two subtrees stay permanently empty (they never re-run,
;; nothing invalidates them). `import` resolves the module to its file and
;; evaluates it if it has not been evaluated yet (spec §4), so the owner is
;; loaded before this body ever runs. Safe to import: eseq.seq-step-tabs is a
;; state/accessor hub with no `effect-buffer`, not one of the four UI roots.
(import eseq.seq-step-tabs :as tabs)

;; Shared scene-bank view state (see that module's header). A state/accessor
;; hub with no effect-buffer, and ui/main.lisp reaches it through this import
;; before the transport body runs.
(import eseq.scene-banks :refer (scene-viewed-bank view-scene-bank! view-new-scene-bank!))
(import eseq.view-kit :refer (open-menu! menu-of nothing listed?))
(import eseq.kinds :refer (transport master engine song project scenes banks launch!
                           delete-scene! launch-quantize-options record-quantize-options))
(import eseq.menus :as menus)
(import eseq.application-menus)

(export transport-stop
        seq-set-scene-launch-quantize
        seq-set-record-quantize
        seq-switch-pattern
        seq-switch-relative
        seq-clone-pattern
        seq-reorder-scene-drop
        scene-push
        scene-push-begin
        scene-push-drag
        scene-push-end
        pattern-control-style
        transport-body
        transport-leading
        transport-scene-strip
        transport-trailing)

;; Identity compat aliases (spec §10 slice 3). Each covers a flat caller that
;; cannot see a qualified name; every one is a function or a singleton
;; instance, both immune to hazard (m):
;;   transport-stop, seq-set-scene-launch-quantize, seq-set-record-quantize,
;;   seq-switch-pattern, seq-reorder-scene-drop, scene-push-begin,
;;   scene-push-drag — evaluated by name from Rust tests in
;;   src/ui/state_values/tests.rs.
;;   scene-push — the push gesture's view singleton, which
;;   ui/capture-fixtures/scene-push-transport.lisp and state_values/tests.rs
;;   write through a local (`(let ((p eseq.transport/scene-push)) (set!
;;   p.target 1))`: a qualified name takes no dotted field).
;;   pattern-control-style — a write-once style `def` read bare by
;;   ui/sequencer.lisp (eseq.sequencer), which may not import a UI root, so
;;   the alias is the supported edge.

;; ── Shared container backgrounds ──
;; `defwidget` names live in their own flat keyspace (hazard e) and are left
;; unrenamed: `transport-btn-bg`, `pattern-pill-bg`, `pattern-pill-btn-bg`,
;; `queued-scene-pill-bg`, `transport-scene-strip-bg` and `save-icon` are named
;; as `:background` strings from other lisp files, capture fixtures and Rust.
;; The `:shader`/`:material` bodies expand OUTSIDE this module (hazard g/h), so
;; every `sdf/*`, `material`, `shadow`, `lighting` reference in them stays FLAT.

(defwidget transport-btn-bg
  :width 1 :height 1
  :paint-margin 0.3
  :state (active)
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.4)
      (material :color (if active :mixer-strip-selected-bg :mixer-strip-bg)
        :shadow (shadow :color (rgba 0 0 0 0.4) :blur 0.08 :offset (vec2 0 0.03))))))

;; Scene-strip variant: the targeted pill grows a rounded lobe out of the
;; shared container as interpolation increases. At zero the lobe is contained
;; entirely by the base, so the idle silhouette is identical to transport-btn-bg.
(defwidget transport-scene-strip-bg
  :width 1 :height 1
  :paint-margin 1.3
  :state (push push-target scene-count)
  :shader
  (let ((amount (clamp push 0.0 1.0))
        (count (max scene-count 1.0))
        ;; Layout geometry in cells: 0.2 outer padding, 2.5-wide scene pills,
        ;; and 0.1 gaps. The trailing spacer (0.2), +/- controls (2.5 each),
        ;; bank selector group (4.2 + 0.12 + 0.45), their 3 gaps, and both
        ;; paddings account for 10.67 cells.
        (total-cells (+ (* count 2.6) 10.67))
        (target-center (+ 0.2 1.25 (* (clamp push-target 0.0 (- count 1.0)) 2.6)))
        (target-x (* aspect (- (* 2.0 (/ target-center total-cells)) 1.0)))
        (base (sdf/rounded-rect width height 0.7))
        (growth (sdf/translate target-x 0.0
          (sdf/rounded-rect
            (mix 0.42 0.82 amount)
            (mix 0.48 1.10 amount)
            (mix 0.32 0.82 amount))))
        (shape (if (< push-target 0.0)
          base
          (sdf/smooth-union (+ 0.001 (* 1.242 amount)) base growth))))
    (sdf/layer
      (sdf/fill shape
        (material :color :mixer-strip-bg
          :shadow (shadow :color (rgba 0 0 0 0.4) :blur 0.08 :offset (vec2 0 0.03)))))))

(defwidget transport-led-bg
  :width 1 :height 1
  :paint-margin 0.3
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.7)
      (material
        :color :mixer-strip-bg))))

(defwidget transport-master-meter
  :width 10.5 :height 0.34
  :paint-margin 0.012
  :state (level)
  :shader
  (let ((lvl (min 1.0 (max 0.0 level)))
        (track (sdf/rounded-rect width height height))
        (green-end (min lvl 0.60))
        (yellow-end (min lvl 0.85))
        (red-end lvl))
    (sdf/layer
      (sdf/fill track
        (material :color (rgba 0.06 0.07 0.08 1)))
      (if (> green-end 0.005)
        (sdf/fill
          (let ((__start 0.0)
                (__end green-end)
                (__half_w (* 0.5 aspect (- __end __start)))
                (__half_h 0.32)
                (__radius (min 0.16 (min __half_h (max __half_w 0.001)))))
            (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end)))))
                  (y (* 0.5 y)))
              (sdf/rounded-rect __half_w __half_h __radius)))
          (material :color (rgba 0.34 0.86 0.40 1)))
        (rgba 0 0 0 0))
      (if (> (- yellow-end 0.60) 0.005)
        (sdf/fill
          (let ((__start 0.60)
                (__end yellow-end)
                (__half_w (* 0.5 aspect (- __end __start)))
                (__half_h 0.32)
                (__radius (min 0.16 (min __half_h (max __half_w 0.001)))))
            (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end)))))
                  (y (* 0.5 y)))
              (sdf/rounded-rect __half_w __half_h __radius)))
          (material :color (rgba 0.86 0.72 0.22 1)))
        (rgba 0 0 0 0))
      (if (> (- red-end 0.85) 0.005)
        (sdf/fill
          (let ((__start 0.85)
                (__end red-end)
                (__half_w (* 0.5 aspect (- __end __start)))
                (__half_h 0.32)
                (__radius (min 0.16 (min __half_h (max __half_w 0.001)))))
            (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end)))))
                  (y (* 0.5 y)))
              (sdf/rounded-rect __half_w __half_h __radius)))
          (material :color (rgba 0.92 0.24 0.22 1)))
        (rgba 0 0 0 0))
      (sdf/fill
        track
        (material :color
          (rgba
            (+ 0.02 (* 0.03 (smoothstep 0.0 0.8 (- y))))
            (+ 0.02 (* 0.03 (smoothstep 0.0 0.8 (- y))))
            (+ 0.03 (* 0.05 (smoothstep 0.0 0.8 (- y))))
            0.18))))))

(defwidget pattern-pill-bg
  :width 1 :height 1
  :state (active push push-target scene)
  :paint-margin 0.3
  :shader
  (let ((push-amount (if (= scene push-target) push 0.0)))
    (sdf/layer
      (sdf/fill (sdf/rounded-rect width height 0.54)
        (material
          :lighting (lighting :edge-min -0.1015 :edge-max 0.9413
            :light (vec3 -0.31 -0.851 1.5) :shininess 51.0)
          :color
          (if (> active 0)
            (let ((base :scene-active-base)
                  (lit (+ 0.06 (* 0.03 diffuse)))
                  (shine (* 0.25 specular)))
              (+ base (rgba lit lit lit 1) (rgba shine shine shine 0)))
            (if hit/hover
              (let ((base :scene-hover-base)
                    (lit (+ 0.06 (* 0.03 diffuse)))
                    (shine (* 0.25 specular)))
                (+ base (rgba lit lit lit 1) (rgba shine shine shine 0)))
              (rgba 0 0 0 0)))))
      (if (> push-amount 0.001)
        (sdf/fill (sdf/rounded-rect width height 0.54)
          (material
            :lighting (lighting :edge-min -0.1015 :edge-max 0.9413
              :light (vec3 -0.31 -0.851 1.5) :shininess 51.0)
            :color
            (let ((amount (clamp push-amount 0.0 1.0))
                  (lit (* amount (+ 0.04 (* 0.05 diffuse))))
                  (shine (* amount 0.24 specular)))
              (* (+ :scene-push-base (rgba (+ lit shine) (+ lit shine) (+ lit shine) 0))
                (rgba 1 1 1 (* 0.88 amount))))))
        (rgba 0 0 0 0)))))

(defwidget queued-scene-pill-bg
  :width 1 :height 1
  :state (push push-target scene)
  :paint-margin 0.3
  :animates true
  :shader
  (let ((pulse (+ 0.5 (* 0.5 (cos (* itime 5.4)))))
        (push-amount (if (= scene push-target) push 0.0))
        (base (+ :scene-queued-base (* :scene-queued-pulse pulse))))
    (sdf/layer
      (sdf/fill (sdf/rounded-rect width height 0.54)
        (material
          :lighting (lighting :edge-min -0.1015 :edge-max 0.9413
            :light (vec3 -0.31 -0.851 1.5) :shininess 51.0)
          :color
          (let ((lit (+ 0.05 (* 0.04 diffuse)))
                (shine (* (+ 0.12 (* 0.16 pulse)) specular)))
            (+ base
              (rgba lit lit lit 1)
              (rgba shine shine shine 0)))))
      (if (> push-amount 0.001)
        (sdf/fill (sdf/rounded-rect width height 0.54)
          (material
            :lighting (lighting :edge-min -0.1015 :edge-max 0.9413
              :light (vec3 -0.31 -0.851 1.5) :shininess 51.0)
            :color
            (let ((amount (clamp push-amount 0.0 1.0))
                  (lit (* amount (+ 0.04 (* 0.05 diffuse))))
                  (shine (* amount 0.20 specular)))
              (* (+ :scene-push-base (rgba (+ lit shine) (+ lit shine) (+ lit shine) 0))
                (rgba 1 1 1 (* 0.72 amount))))))
        (rgba 0 0 0 0)))))


(defwidget scene-bank-playing-indicator
  :width 0.45 :height 0.45
  :paint-margin 0.15
  :animates true
  :shader
  (let ((pulse (+ 0.58 (* 0.42 (cos (* itime 4.8))))))
    (sdf/fill (sdf/circle (* 0.34 (+ 0.82 (* 0.18 pulse))))
      (material :color (* :scene-bank-indicator (rgba 1 1 1 pulse))))))

(defwidget pattern-pill-btn-bg
 :width 1 :height 1
  :state (active)
  :paint-margin 0.3
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height height)
      (material
        :lighting (lighting :edge-min -0.1015 :edge-max 0.9413
          :light (vec3 -0.31 -0.851 1.5) :shininess 51.0)
        :color
        (if (> active 0)
          (let ((base :scene-action-base)
                (lit (+ 0.06 (* 0.03 diffuse)))
                (shine (* 0.25 specular)))
            (+ base (rgba lit lit lit 1) (rgba shine shine shine 0)))
          (if hit/hover
            (let ((base :scene-hover-base)
                  (lit (+ 0.06 (* 0.03 diffuse)))
                  (shine (* 0.25 specular)))
              (+ base (rgba lit lit lit 1) (rgba shine shine shine 0)))
            (rgba 0 0 0 0)))))))

(defwidget add-track-icon
  :width 2.5 :height 2.5
  :paint-margin 0.5
  :state (active)
  :shader
  (let ((fg-col (if (= active 1) :icon-active-fg :icon-fg)))
    (sdf/layer
        (rgba 0 0 0 0)
      (sdf/fill (sdf/rounded-rect 0.12 0.72 0.05)
        (material :color fg-col))
      (sdf/fill (sdf/rounded-rect 0.72 0.12 0.05)
        (material :color fg-col)))))

(defwidget save-icon
  :width 2.8 :height 1.4
  :paint-margin 0.5
  :state (active)
  :shader
  (let ((fg-col :save-icon-fg)
      (bg-col (if (= active 1)
          :mixer-strip-selected-bg
          :mixer-strip-bg
          )))
    (sdf/layer
      (sdf/fill
        (sdf/rounded-rect width height 0.4)
        (material :color bg-col))
      
      (sdf/fill
        (sdf/translate 0.0 -0.60
          (sdf/rounded-rect 0.42 0.32 0.12))
        (material :color fg-col))
      (sdf/fill
        (sdf/translate 0.22 -0.60
          (sdf/rounded-rect 0.14 0.26 0.1))
        (material :color bg-col))
      (sdf/fill
        (sdf/translate 0.0 0.38
          (sdf/rounded-rect 0.48 0.33 0.12))
        (material :color fg-col)))))

(defwidget samples-sidebar-icon
  :width 2.8 :height 1.4
  :paint-margin 0.5
  :state (active)
  :shader
  (let ((fg-col :fg)
      (muted-col :gray)
      (bg-col (if (= active 1)
          :transparent
          :transparent
          ))
      (panel-col (if (= active 1) fg-col muted-col)))
    (sdf/layer
      (sdf/fill
        (sdf/translate -0.38 0.0
          (sdf/rounded-rect 0.10 0.55 0.08))
        (material :color panel-col))
      (sdf/fill
        (sdf/translate 0.18 0.34
          (sdf/rounded-rect 0.34 0.08 0.03))
        (material :color panel-col))
      (sdf/fill
        (sdf/translate 0.18 0.0
          (sdf/rounded-rect 0.34 0.08 0.03))
        (material :color panel-col))
      (sdf/fill
        (sdf/translate 0.18 -0.34
          (sdf/rounded-rect 0.34 0.08 0.03))
        (material :color panel-col)))))

(defwidget mix-panel-icon
  :width 2.8 :height 1.4
  :paint-margin 0.5
  :state (active)
  :shader
  (let ((fg-col :fg)
      (muted-col :gray)
      (bg-col (if (= active 1)
          :mixer-strip-selected-bg
          :transparent
          ))
      (panel-col (if (= active 1) fg-col muted-col)))
    (sdf/layer
      (sdf/fill
        (sdf/translate 0.0 -0.34
          (sdf/rounded-rect 0.56 0.10 0.08))
        (material :color panel-col))
      (sdf/fill
        (sdf/translate -0.34 0.14
          (sdf/rounded-rect 0.08 0.34 0.03))
        (material :color panel-col))
      (sdf/fill
        (sdf/translate 0.0 0.14
          (sdf/rounded-rect 0.08 0.34 0.03))
        (material :color panel-col))
      (sdf/fill
        (sdf/translate 0.34 0.14
          (sdf/rounded-rect 0.08 0.34 0.03))
        (material :color panel-col)))))

(defwidget fx-panel-icon
  :width 2.8 :height 1.4
  :paint-margin 0.5
  :state (active)
  :shader
  (let ((fg-col :fg)
      (muted-col :gray)
      (bg-col (if (= active 1)
          :mixer-strip-selected-bg
          :transparent
          ))
      (panel-col (if (= active 1) fg-col muted-col)))
    (sdf/layer
      (sdf/fill
        (sdf/translate 0.0 0.34
          (sdf/rounded-rect 0.56 0.10 0.08))
        (material :color panel-col))
      (sdf/fill
        (sdf/translate -0.34 -0.14
          (sdf/rounded-rect 0.08 0.34 0.03))
        (material :color panel-col))
      (sdf/fill
        (sdf/translate 0.0 -0.14
          (sdf/rounded-rect 0.08 0.34 0.03))
        (material :color panel-col))
      (sdf/fill
        (sdf/translate 0.34 -0.14
          (sdf/rounded-rect 0.08 0.34 0.03))
        (material :color panel-col)))))

(defwidget session-view-icon
  :width 2.8 :height 1.4
  :paint-margin 0.5
  :state (active)
  :shader
  (let ((fg-col :fg)
      (muted-col :gray)
      (bg-col :transparent)
      (line-col (if (= active 1) fg-col muted-col)))
    (sdf/layer
      (sdf/fill
        (sdf/rounded-rect width height 0.4)
        (material :color bg-col))
      (sdf/fill
        (sdf/translate -0.34 0.22
          (sdf/rounded-rect 0.09 0.42 0.03))
        (material :color line-col))
      (sdf/fill
        (sdf/translate 0.0 0.08
          (sdf/rounded-rect 0.09 0.56 0.03))
        (material :color line-col))
      (sdf/fill
        (sdf/translate 0.34 -0.08
          (sdf/rounded-rect 0.09 0.72 0.03))
        (material :color line-col)))))

(defwidget arrangement-view-icon
  :width 2.8 :height 1.4
  :paint-margin 0.5
  :state (active)
  :shader
  (let ((fg-col :fg)
      (muted-col :gray)
      (bg-col :transparent)
      (line-col (if (= active 1) fg-col muted-col)))
    (sdf/layer
      (sdf/fill
        (sdf/rounded-rect width height 0.4)
        (material :color bg-col))
      (sdf/fill
        (sdf/translate -0.20 -0.32
          (sdf/rounded-rect 0.54 0.08 0.03))
        (material :color line-col))
      (sdf/fill
        (sdf/translate -0.08 0.0
          (sdf/rounded-rect 0.66 0.08 0.03))
        (material :color line-col))
      (sdf/fill
        (sdf/translate 0.04 0.32
          (sdf/rounded-rect 0.78 0.08 0.03))
        (material :color line-col)))))

(defwidget transport-tool-chip-bg
  :width 1 :height 1
  :state (active)
  :paint-margin 0.3
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height height)
      (material
        :color (if (= active 1)
                 (rgba 0.00 0.35 0.82 1.0)
                 (rgba 0.18 0.18 0.20 1.0))
        :shadow (shadow :color (rgba 0 0 0 0.42) :blur 0.06 :offset (vec2 0 0.02))))))

;; ── Button widgets — icons scaled 2x ──

(defwidget stop-icon
  :width 2.5 :height 1.8
  :paint-margin 0.5
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect 0.44 0.44 0.05)
      (material :color :icon-fg))))

;; Stop returns the arrangement insertion/start marker to the beginning even
;; when playback is already stopped. The cursor mirror makes the next Play
;; start at beat zero; -1 leaves no track-specific cursor line behind.
(def transport-stop ()
  (when transport.playing (set! transport.playing false))
  (eseq.arrangement/set-cursor 0 -1))

(defwidget play-icon
  :width 2.5 :height 1.8
  :paint-margin 0.5
  :state (active)
  :shader
  (let ((fg-col (if (= active 1) :icon-active-fg :icon-fg)))
    (sdf/layer
      (if (= active 1)
        (sdf/fill (sdf/rounded-rect (* 0.75 height) (* 0.75 height) 0.4)
          (material
            :lighting (lighting :edge-min -0.1015 :edge-max 0.9413
              :light (vec3 -0.31 -0.851 1.3) :shininess 51.0)
            :color
            (let ((base :play-active-base)
                  (lit (+ 0.025 (* 0.05 diffuse)))
                  (shine (* 0.18 specular)))
              (+ base (rgba lit lit lit 1) (rgba shine shine shine 0)))))
        (rgba 0 0 0 0))
      (sdf/fill
        (let ((p1x -0.35) (p1y -0.5) (p2x -0.35) (p2y 0.5) (p3x 0.55) (p3y 0.0))
          (let ((d1 (- (* (- p2x p1x) (- y p1y)) (* (- p2y p1y) (- x p1x))))
                (d2 (- (* (- p3x p2x) (- y p2y)) (* (- p3y p2y) (- x p2x))))
                (d3 (- (* (- p1x p3x) (- y p3y)) (* (- p1y p3y) (- x p3x)))))
            (max (max d1 d2) d3)))
        (material :color fg-col)))))

;; Back to Arrangement: a theme-accent tile with a play triangle
;; and three arrangement lanes beside it. Lights the moment a manual launch
;; overrides the arrangement; fully transparent while nothing is latched.
(defwidget back-to-arrangement-icon
  :width 2.5 :height 1.8
  :paint-margin 0.5
  :state (active)
  :shader
  (if (= active 1)
    (sdf/layer
      (sdf/fill (sdf/rounded-rect (* 0.75 height) (* 0.75 height) 0.4)
        (material
          :lighting (lighting :edge-min -0.1015 :edge-max 0.9413
            :light (vec3 -0.31 -0.851 1.3) :shininess 51.0)
          :color
          (let ((lit (+ 0.04 (* 0.10 diffuse)))
                (shine (* 0.20 specular)))
            (+ :arrangement-return-base
               (rgba lit (* 0.6 lit) (* 0.3 lit) 1)
               (rgba shine shine shine 0)))))
      (sdf/fill
        (let ((p1x -0.62) (p1y -0.34) (p2x -0.62) (p2y 0.34) (p3x -0.10) (p3y 0.0))
          (let ((d1 (- (* (- p2x p1x) (- y p1y)) (* (- p2y p1y) (- x p1x))))
                (d2 (- (* (- p3x p2x) (- y p2y)) (* (- p3y p2y) (- x p2x))))
                (d3 (- (* (- p1x p3x) (- y p3y)) (* (- p1y p3y) (- x p3x)))))
            (max (max d1 d2) d3)))
        (material :color :arrangement-return-fg))
      (sdf/fill (sdf/translate 0.38 -0.28 (sdf/rounded-rect 0.28 0.055 0.03))
        (material :color :arrangement-return-fg))
      (sdf/fill (sdf/translate 0.38 0.0 (sdf/rounded-rect 0.28 0.055 0.03))
        (material :color :arrangement-return-fg))
      (sdf/fill (sdf/translate 0.38 0.28 (sdf/rounded-rect 0.28 0.055 0.03))
        (material :color :arrangement-return-fg)))
    (rgba 0 0 0 0)))

(defwidget rec-icon
  :width 2.5 :height 1.8
  :paint-margin 0.5
  :state (active)
  :shader
  (let ((fg-col (if (= active 1) :icon-active-fg :record-idle-fg)))
    (sdf/layer
      (if (= active 1)
        (sdf/fill (sdf/rounded-rect (* 0.75 height) (* 0.75 height) 0.4)
          (material
            :lighting (lighting :edge-min -0.1015 :edge-max 0.9413
              :light (vec3 -0.31 -0.851 1.5) :shininess 51.0)
            :color
            (let ((base :record-active-base)
                  (lit (+ 0.06 (* 0.40 diffuse)))
                  (shine (* 0.25 specular)))
              (+ base (rgba lit 0 0 1) (rgba shine shine shine 0)))))
        (rgba 0 0 0 0))
      (sdf/fill (sdf/circle 0.4)
        (material :color fg-col)))))

;; A label field the host has not published yet reads "": show its default.
(def label-or (text default) (if (= text "") default text))
(def launch-quantize () (label-or transport.launch-quantize "off"))

(def seq-set-scene-launch-quantize (value)
  (set! transport.launch-quantize value))

(def seq-set-record-quantize (value)
  (set! transport.record-quantize value))

(def seq-switch-pattern (idx)
  (host-command "switch-pattern" (dict :idx idx :quantize (launch-quantize))))

;; Resolve relative movement in the host when commands are drained. Several
;; button presses in one MIDI batch must advance from each other's targets.
(def seq-switch-relative (delta)
  (host-command "switch-pattern-relative" (dict :delta delta :quantize (launch-quantize))))

;; ── Menus ──
;; Each context menu is a `:key ()` singleton with `open`, `at` (where it
;; opens) and what it targets (eseq.view-kit's open-menu! and menu-of).

;; ── Scene banks ──
;; The viewed bank lives in eseq.scene-banks so the mixer clip grid
;; (scene-banks spec 10.1) shares it with this strip. A bank or scene a menu
;; or the rename holds can go stale (a project load, an undo, an edit from
;; elsewhere) while it is open: the actions check it is still listed before
;; sending its id.

(def scene-bank-labels () (map (lambda (b) b.label) (banks)))

(def bank-labelled (text) (first (filter (lambda (b) (= b.label text)) (banks))))

;; A bank's size, and the index its first scene has (or would have: banks
;; are consecutive spans of the scene list, and a bank can be empty).
(def bank-size (b) (len b.scenes))
(def bank-offset (b)
  (reduce |n x| (if (< x.index b.index) (+ n (bank-size x)) n) 0 (banks)))
(def bank-full? (b) (>= (bank-size b) 24))

(def select-scene-bank (text)
  (if (= text "New bank")
    (do
      ;; create-scene-bank appends; the view lands on it once the host lists it.
      (view-new-scene-bank!)
      (host-command "create-scene-bank" (dict)))
    (let ((b (bank-labelled text)))
      (when b (view-scene-bank! b)))))

;; The viewed bank's Rename / Delete menu, and its inline rename (`bank` is
;; the bank being renamed, nil when none is).
(def-kind bank-ops-menu
  :key ()
  :state ((open false)
          (at :point :default nil)))

(def-kind bank-rename
  :key ()
  :state ((bank bank :default nil)
          (draft "")))

(def open-scene-bank-ops-menu (event)
  (open-menu! bank-ops-menu event))

(def begin-scene-bank-rename ()
  (let ((b (scene-viewed-bank)))
    (set! bank-ops-menu.open false)
    (set! bank-rename.draft b.name)
    (set! bank-rename.bank b)))

;; Stored names are trimmed host-side, so compare against the trimmed draft;
;; a no-op commit would otherwise surface a host error status.
(def scene-bank-rename-changed? ()
  (let ((b bank-rename.bank))
    (and b (listed? b (banks)) (not (= (string-trim bank-rename.draft) b.name)))))

(def finish-scene-bank-rename (commit)
  (let ((b bank-rename.bank))
    (when b
      (when (and commit (scene-bank-rename-changed?))
        (host-command "rename-scene-bank" (dict :bank-id b.bid :name bank-rename.draft)))
      (set! bank-rename.bank nil)
      (set! bank-rename.draft ""))))

;; Deleting a bank moves its scenes into its neighbor (the next bank for the
;; first one, else the previous), which must have room for them.
(def scene-bank-neighbor (b)
  (let ((all (banks)))
    (if (<= (len all) 1)
      nil
      (nth all (if (= b.index 0) 1 (- b.index 1))))))

(def scene-viewed-bank-deletable? ()
  (let ((b (scene-viewed-bank)))
    (let ((target (if b (scene-bank-neighbor b) nil)))
      (and target (<= (+ (bank-size b) (bank-size target)) 24)))))

(def delete-viewed-scene-bank ()
  (when (scene-viewed-bank-deletable?)
    (let ((b (scene-viewed-bank)))
      (set! bank-ops-menu.open false)
      (view-scene-bank! (scene-bank-neighbor b))
      (host-command "delete-scene-bank" (dict :bank-id b.bid)))))

(def scene-bank-ops-context-menu ()
  (menu-of bank-ops-menu
    (menu-item "Rename bank"
      :key "scene-bank-rename-action"
      :on-select (lambda (event) (begin-scene-bank-rename)))
    (menu-item "Delete bank"
      :key "scene-bank-delete-action"
      :disabled (not (scene-viewed-bank-deletable?))
      :on-select (lambda (event) (delete-viewed-scene-bank)))))

(def scene-bank-selector (b)
  (box :key "scene-bank-selector"
    :width 4.2 :height 0.8
    :on-right-click (lambda (event) (open-scene-bank-ops-menu event))
    (if bank-rename.bank
      (text-input :key "scene-bank-rename-input"
        :width 4.2 :height 0.8 :font-size 8
        :value bank-rename.draft
        :auto-focus true
        :select-all-on-focus true
        :on-change (lambda (name) (set! bank-rename.draft name))
        :on-submit (lambda () (finish-scene-bank-rename true))
        :on-cancel (lambda () (finish-scene-bank-rename false))
        :on-blur (lambda () (finish-scene-bank-rename true)))
      (dropdown :key "scene-bank-dropdown"
        :value b.label
        :options (append (scene-bank-labels) (list "New bank"))
        :on-change select-scene-bank
        :bg-color :mixer-strip-bg
        :border-color :mixer-strip-border
        :badge-color :transparent
        :width 4.2 :height 0.5 :font-size 10))))

;; A new scene at the end of the viewed bank.
(def seq-clone-pattern ()
  (let ((b (scene-viewed-bank)))
    (host-command "clone-pattern"
      (dict :bank-id b.bid :insert-position (+ (bank-offset b) (bank-size b))))))

;; A scene's "Move to bank" menu.
(def-kind scene-bank-menu
  :key ()
  :state ((open false)
          (at :point :default nil)
          (scene scene :default nil)))

(def open-scene-bank-menu (event s)
  (set! scene-bank-menu.scene s)
  (open-menu! scene-bank-menu event))

(def scene-bank-is-source? (b)
  (let ((s scene-bank-menu.scene))
    (and s (= s.bank b))))

(def move-scene-to-scene-bank (b)
  (let ((s scene-bank-menu.scene))
    (unless (or (scene-bank-is-source? b) (bank-full? b))
      (set! scene-bank-menu.open false)
      (when (and (listed? s (scenes)) (listed? b (banks)))
        (host-command "move-scene-to-scene-bank" (dict :scene s.index :bank-id b.bid))))))

(def scene-bank-context-menu ()
  (apply menu-of scene-bank-menu
    (each (banks) |b|
      (menu-item (str "Move to bank " b.label)
        :key (str "scene-bank-move-" b.bid)
        :disabled (or (scene-bank-is-source? b) (bank-full? b))
        :on-select (lambda (event) (move-scene-to-scene-bank b))))))

(def seq-reorder-scene-drop (event)
  (let ((source event.payload.scene)
        (target event.target.scene))
    (unless (= source target)
      (host-command "reorder-scene" (dict :source source :target target)))))

;; Shift gesture state is UI-local and intentionally ephemeral. The modifier
;; is sampled only on pointer-down; releasing Shift while still holding the
;; mouse cannot turn the gesture into a reorder operation. `target` is the
;; pushed scene's index (-1: none).
(def-kind scene-push
  :key ()
  :state ((target -1)
          (value 1.0)
          (start-y 0.0)
          (from-source false)))

(def reset-scene-push! ()
  (set! scene-push.target -1)
  (set! scene-push.from-source false)
  (set! scene-push.value 1.0))

;; Pointer-down on scene s's pill: Shift pushes toward it, Command pushes
;; from it; a plain press launches it.
(def scene-push-begin (s event)
  (if (or event.shift event.cmd)
    (let ((value (if event.cmd 0.0 1.0)))
      (set! scene-push.target s.index)
      (set! scene-push.from-source (if event.cmd true false))
      (set! scene-push.value value)
      (set! scene-push.start-y event.y)
      (host-command "scene-push-begin" (dict :target-scene s.index :value value)))
    (launch! s)))

(def scene-push-drag (s event)
  (when (= scene-push.target s.index)
    (let ((value (if scene-push.from-source
          (clamp (* 0.14 (- event.y scene-push.start-y)) 0.0 1.0)
          (clamp (+ 1.0 (* 0.14 (- scene-push.start-y event.y))) 0.0 1.0))))
      (set! scene-push.value value)
      (host-command "scene-push-set-value" (dict :value value)))))

(def scene-push-end (s event)
  (when (= scene-push.target s.index)
    (host-command "scene-push-end" (dict))
    (reset-scene-push!)))

(def transport-icon-style
  (ui/style
    :pressed (dict
      :scale 1.08
      :transition (dict :scale 0.12 :ease :smoothstep))
    :hover (dict
      :brightness 1.10
      :transition (dict :brightness 0.12 :ease :smoothstep))))

(def pattern-control-style
  (ui/style
    :pressed (dict
      :scale 1.06
      :transition (dict :scale 0.10 :ease :smoothstep))
    :hover (dict
      :brightness 1.12
      :transition (dict :brightness 0.12 :ease :smoothstep))))

;; Saved per scene; defscene supplies persistence, targeted repaint, and undo.
(defscene scene-transpose 0)

;; The transpose picker's "apply to" menu: the value and bank it opened on.
(def-kind transpose-menu
  :key ()
  :state ((open false)
          (at :point :default nil)
          (value 0)
          (bank bank :default nil)))

(def open-transpose-menu (event)
  (set! transpose-menu.value scene-transpose)
  (set! transpose-menu.bank (scene-viewed-bank))
  (open-menu! transpose-menu event))

(def apply-transpose-menu (scope)
  (let ((b transpose-menu.bank))
    (set! transpose-menu.open false)
    (when (listed? b (banks))
      (host-command "apply-scene-transpose"
        (dict :scope scope :bank-id b.bid :value transpose-menu.value)))))

(def transpose-context-menu ()
  (menu-of transpose-menu
    (menu-item "Apply to all scenes in this bank"
      :key "transpose-apply-bank"
      :on-select (lambda (event) (apply-transpose-menu "bank")))
    (menu-item "Apply to all scenes in all banks"
      :key "transpose-apply-all-banks"
      :on-select (lambda (event) (apply-transpose-menu "all-banks")))))

;; One command catalog feeds the native macOS menu and the toolbar fallback.
;; Native availability is published only after the application installs a menu;
;; headless captures and other platforms retain the toolbar menus. `open` is
;; the open menu's id ("" for none).
(def-kind app-menu
  :key ()
  :state ((open "")
          (at :point :default nil)))

(def open-file-menu (event)
  (open-application-menu "File" event))

(def open-application-menu (name event)
  (set! app-menu.at event.at)
  (set! app-menu.open name))

(def close-application-menu ()
  (set! app-menu.open ""))

;; Save follows the buffer in front: one whose mode saves itself (an expr
;; card's edit buffer commits its body) takes the chord; everything else
;; saves the project.
(def file-menu-save ()
  (if (current-buffer-saves-itself?)
    (save-buffer)
    (host-command "project-save-open" (dict :mode "save"))))

(def file-menu-save-as ()
  (host-command "project-save-open" (dict :mode "save-as")))

(def file-menu-open-project ()
  (do
    (if (not eseq.seq-core-state/samples-sidebar-visible)
      (eseq.seq-panels/seq-toggle-samples-sidebar)
      nil)
    (eseq.browser/open-project-browser)))

(def application-menu-names ()
  (map (lambda (menu) (get menu :id))
    (filter (lambda (menu) (not (get menu :native-only))) (menus/current-menus))))

(def application-menu (id)
  (let ((matches (filter (lambda (menu) (= (get menu :id) id)) (menus/current-menus))))
    (if (> (len matches) 0) (nth matches 0) nil)))

(def application-menu-items (id) (get (application-menu id) :items))
(def run-menu-action (name id) (menus/activate id))

(def application-menu-row (item)
  (if item
    (menu-item (get item :label) :key (get item :id)
      :disabled (not (get item :enabled))
      :checked (get item :checked)
      :shortcut (menus/shortcut-label (get item :shortcut))
      :on-select (lambda (event) (menus/activate (get item :id)))
      (if (get item :items) (map application-menu-row (get item :items)) (list)))
    (menu-separator)))

(def application-context-menu (name)
  (context-menu :is-open (= app-menu.open name)
    :anchor app-menu.at
    :on-close (lambda () (close-application-menu))
    (map application-menu-row (application-menu-items name))))

(def file-context-menu () (application-context-menu "File"))

(def application-menu-button (name)
  (box :height 1.4 :padding 0.35 :corner-radius 12
    :background-color (if (= app-menu.open name) :mixer-strip-selected-bg :mixer-strip-bg)
    :style transport-icon-style
    :on-click (lambda (event) (if (get (application-menu name) :enabled) (open-application-menu name event) nil))
    (v-stack :align :center :height :fill
      (label (get (application-menu name) :label) :font-size 11 :color :white :bg :transparent))))

;; ── Transport layout ──

;; A label on a pill: white while on (a value or a #' binding), else gray.
(def pill-label (text on &key (font 9))
  (label text :font-size font :color :gray :active on :active-color :white
    :hover-color :white :bg :transparent))

;; Widget-only buffer: take the shared sequencer keymap (was an implicit host default).
(set-buffer-mode-for "*transport*" "eseq.sequencer-keys/sequencer-keys")
;; The transport bar is assembled from four overridable functions so a
;; package can re-flow it (e.g. a two-row transport with the scene strip
;; underneath) without retyping the controls: `transport-leading` (view
;; buttons, menus, playback, clock, meters, cpu), `transport-scene-strip`
;; (the scene pills), `transport-trailing` (context menus and the session/
;; arrangement pair), and `transport-body` which lays them out. The first
;; and third return child lists; stacks splice lists into their children.
(def transport-leading ()
  (list
    
    (subtree :key "transport-samples-sidebar-button"
      (samples-sidebar-icon
        :on-click |x y r| (eseq.seq-panels/seq-toggle-samples-sidebar)
        :style transport-icon-style
        :active (if eseq.seq-core-state/samples-sidebar-visible 1 0)))
    
    (subtree :key "transport-mixer-panel-button"
      (mix-panel-icon
        :on-click |x y r| (eseq.seq-panels/seq-toggle-mixer-panel)
        :style transport-icon-style
        :active (if eseq.seq-core-state/mixer-panel-visible 1 0)))
    
    (subtree :key "transport-fx-panel-button"
      (fx-panel-icon
        :on-click |x y r| (eseq.seq-panels/seq-toggle-fx-panel)
        :style transport-icon-style
        :active (if eseq.seq-core-state/lower-panel-visible 1 0)))
    
    (box :width 2)
    ;; A plain box, not an SDF icon: SDF widgets call `:on-click` with bare
    ;; |x y r| args, while a box passes the event map the menu anchors on.
    (subtree :key "transport-application-menus"
      (if (native-menu-installed?)
        (nothing)
        (h-stack :gap 0.2 :align :center
          (map (lambda (name)
              (subtree :key (str "transport-" name "-menu-button")
                (application-menu-button name)))
            (application-menu-names)))))
    
    ;; Transport buttons in a shared rounded-rect container
    (box :key "transport-playback-controls" :background-color :mixer-strip-bg :corner-radius 72 :padding 0.015 :height 1.4
      (h-stack :gap 0.2 :align :center
        (subtree :key "transport-stop-button"
          (box :width 2.5
            :on-click |x y r| (transport-stop)
            (stop-icon)))
        (box :width 2.5
          :on-click |x y r| (toggle! transport.playing)
          (play-icon :active #'transport.playing))
        (box :width 2.5
          :on-click |x y r| (toggle! transport.recording)
          (rec-icon :active #'transport.recording))
        (subtree :key "transport-master-record-button"
          (box :debug-name "transport-master-record-button"
            :width 4.2 :height 1.1
            :background "pattern-pill-bg"
            :active #'master.recording
            :style transport-icon-style
            :on-click |x y r| (toggle! master.recording)
            (v-stack :align :center
              (pill-label "WAV" #'master.recording :font 10))))
        ;; Back to Arrangement (unified-transport spec; Ableton semantics):
        ;; lights the moment a manual launch overrides the arrangement,
        ;; SURVIVES transport stop, and clicking hands the latched lanes
        ;; back to the arrangement. The box is ALWAYS laid out (the icon
        ;; binds the latch, so a flip only repaints); the icon is
        ;; transparent while nothing is latched, and the click guards at
        ;; event time.
        (subtree :key "transport-back-to-arrangement"
          (box :debug-name "transport-back-to-arrangement"
            :width 2.5 :height 1.4
            :style transport-icon-style
            :on-click |x y r| (when song.manual-latch (set! song.manual-latch false))
            (back-to-arrangement-icon :active #'song.manual-latch)))))
    
    ;; Single continuous LED panel
    (box :background-color :mixer-strip-bg :corner-radius 64 :height 1.4 :width 77
      (h-stack :align :center
        (subtree :key "transport-clock"
          (h-stack :gap 0 :align :center :padding-left 0.5 :padding-right 0.5
            (transport-clock
              ;; One transport (docs/unified-transport-spec.md 4/8): the
              ;; parked arrangement cursor while stopped, the live absolute
              ;; arrangement clock during playback/capture.
              :playhead #'transport.position
              :song-position-beats
              (if (= song.mode "stopped") #'song.cursor #'song.position)
              :use-song-position true
              :font-size 15 :width 10 :height 1.2
              :color :clock-fg
              :bg :transparent)
            (label "" :width 1 :bg :transparent)
            (number-picker :value transport.bpm :min 20 :max 300 :decimals 1
              :key "transport-bpm"
              :noui true
              :font-size 15
              :text-color :clock-fg
              :on-change (lambda (v) (set! transport.bpm (floor v)))
              :width 7 :height 1.2)
            (subtree :key "transport-scene-transpose"
              (number-picker :value scene-transpose
                :key "transport-scene-transpose-picker"
                :debug-name "transport-scene-transpose"
                :on-right-click open-transpose-menu
                :min -48 :max 48 :step 1 :decimals 0 :unit "st"
                :noui true :font-size 15 :text-color :clock-fg
                :on-change (lambda (v) (set! scene-transpose (round v)))
                :width 5.5 :height 1.2))
            (subtree :key "transport-scene-launch-quantize"
              (dropdown
                :bg-color :mixer-strip-bg
                :border-color :mixer-strip-selected-bg
                :badge-color :transparent
                :key "transport-scene-launch-quantize-dropdown"
                :debug-name "transport-scene-launch-quantize"
                :value (launch-quantize)
                :options launch-quantize-options
                :on-change seq-set-scene-launch-quantize
                :width 5.2 :height 1.15 :font-size 9))
            (box :width 1.0)
            (subtree :key "transport-record-quantize"
              (dropdown
                :bg-color :mixer-strip-bg
                :border-color :mixer-strip-selected-bg
                :badge-color :transparent
                :key "transport-record-quantize-dropdown"
                :debug-name "transport-record-quantize"
                :value (label-or transport.record-quantize "1/16")
                :options record-quantize-options
                :on-change seq-set-record-quantize
                :width 5.2 :height 1.15 :font-size 9))
            (box :width 1.5)
            (subtree :key "transport-metronome-toggle"
              (box :debug-name "transport-metronome-toggle"
                :width 3.4 :height 1.1
                :background "pattern-pill-bg"
                :on-click |x y r| (toggle! transport.metronome)
                (v-stack :align :center
                  (pill-label "MET" #'transport.metronome))))
            ;; Roll mode (docs/rolling-core-spec.md 8): toggle + live rate
            ;; display. Rate keys 1-8 switch the rate while roll mode is on.
            (subtree :key "transport-roll-toggle"
              (box :debug-name "transport-roll-toggle"
                :width 5.5 :height 1.1
                :background-color (if transport.sequence-rolling
                  '(rgba 0.72 0.10 0.12 1)
                  "pattern-pill-bg")
                :on-click |x y r| (toggle! transport.roll-mode)
                (h-stack :align :baseline :gap 0.3
                  (box :width 0.2)
                  (pill-label "ROLL" #'transport.roll-mode)
                  (label (if transport.roll-mode transport.roll-rate "")
                    :font-size 9
                    :color '(rgba 0.63 0.88 0.41 1)
                    :bg :transparent))))))
        (v-stack :gap 0.08 :padding 0.05
          (label "L"
            :font-size 5 :width 0.9
            :v-align :center
            :color '(rgba 0.63 0.88 0.41 1)
            :bg :transparent)
          
          (label "R"
            :font-size 5 :width 0.9
            :v-align :center
            :color '(rgba 0.63 0.88 0.41 1)
            :bg :transparent)          )
        
        
        (v-stack :gap 0.08 :padding 0.05
          (h-stack :gap 0.25
            
            (v-stack
              (box :height 0.0)
              (subtree :key "master-meter-l"
                (transport-master-meter :level #'master.peak-l))))
          (h-stack :gap 0.25 :align :center
            
            (v-stack (box :height 0.1)
              (subtree :key "master-meter-r"
                (transport-master-meter :level #'master.peak-r)))))
        (subtree :key "transport-cpu"
          (h-stack :gap 0 :align :center :padding 0.4
            (box :height 2.7
              (label "cpu"
                :v-align :center
                :font-size 12 :width 3.0
                :color :gray :active #'engine.overloaded :active-color :red
                :bg :transparent))
            (number-label :key "transport-cpu-value"
              :value #'engine.cpu-load
              :decimals 0 :min-integer-digits 2 :suffix "%"
              :font-size 12 :width 2.0 :height 1
              :color :dim :active #'engine.overloaded :active-color :red
              :bg :transparent)))
        ;; The latency planner aligns every route to this delay. A latent FX
        ;; therefore delays the whole project, not only the track holding it.
        (subtree :key "transport-output-latency"
          (h-stack :gap 0 :align :baseline :padding 0.5
            (number-label :key "transport-output-latency-value"
              :value #'engine.latency-ms
              :decimals 1 :min-integer-digits 2 :suffix "ms"
              :font-size 12 :width 3.5 :height 1
              :color :gray
              :bg :transparent)))))
    
  ))

;; One scene's pill: lit while it plays (bindings: launches only repaint),
;; pulsing while queued. Its index addresses the host's commands.
(def scene-pill (s)
  (let ((scene s.index))
    (box :key (str "transport-scene-pill-" scene)
      :width 2.5 :height 1.1
      ;; queued-scene-pill-bg has no `active` state, so it takes no binding.
      :background (if s.queued "queued-scene-pill-bg" "pattern-pill-bg")
      :active (if s.queued (if s.active 1 0) #'s.active)
      :push #'scene-push.value
      :push-target scene-push.target
      :scene scene
      :style pattern-control-style
      :capture-pointer true
      :drag-type "transport-scene"
      :drag-modifier :none
      :drag-payload (dict :scene scene)
      :drop-types (list "transport-scene")
      :drop-meta (dict :scene scene)
      :drop-hover-border-color :mixer-strip-selected-border
      :on-drop seq-reorder-scene-drop
      :on-right-click (lambda (event) (open-scene-bank-menu event s))
      :on-mouse-down (lambda (event) (scene-push-begin s event))
      :on-drag (lambda (event) (scene-push-drag s event))
      :on-mouse-up (lambda (event) (scene-push-end s event))
      (v-stack :align :center
        (label (fmt " {} " s.number)
          :font-size 11
          :color (if s.queued :scene-active-fg :gray)
          :active #'s.active :active-color :scene-active-fg
          :hover-color :white
          :bg :transparent)))))

;; A "+" / "-" button of the strip; `enabled` lights it and lets it click.
(def scene-strip-button (key text enabled action)
  (box :key key :background "pattern-pill-btn-bg"
    :width 2.5 :height 1.1 :active true
    :style (if enabled pattern-control-style nil)
    :on-click |x y r| (action)
    (v-stack :align :center
      (label text
        :font-size 12
        :color (if enabled :white :dark-gray)
        :bg :transparent))))

;; The playing scene is in another bank than b.
(def scene-playing-in-other-bank? (b) (not b.playing))

(def scene-strip (b)
  (let ((offset (bank-offset b))
        (size (bank-size b))
        (deletable (and (> (len project.scenes) 1) b.playing)))
    (box :background "transport-scene-strip-bg"
      :corner-radius 64
      :key "transport-scene-strip"
      :debug-name "transport-scene-strip"
      :push #'scene-push.value
      :push-target (if (and (>= scene-push.target offset)
                            (< scene-push.target (+ offset size)))
                     (- scene-push.target offset)
                     -1)
      :scene-count size
      :padding 0.2 :height 1.4
      (h-stack :gap 0.1 :align :center
        (each b.scenes |s| (scene-pill s))
        (label "" :width 0.2 :bg :transparent)
        (scene-strip-button "scene-bank-add" "+" (not (bank-full? b))
          (lambda ()
            (if (bank-full? b)
              (status "This scene bank is full (24 scenes maximum)")
              (seq-clone-pattern))))
        (scene-strip-button "scene-bank-delete" "-" deletable
          (lambda () (when deletable (delete-scene! transport.scene))))
        (h-stack :gap 0.12 :align :center
          (box :width 0.5)
          (scene-bank-selector b)
          (if (scene-playing-in-other-bank? b)
            (scene-bank-playing-indicator
              :debug-name "scene-bank-playing-other-indicator")
            (box :width 0.45 :height 0.45 :bg :transparent)))
        (scene-bank-context-menu)
        (scene-bank-ops-context-menu)))))

;; Pattern pills in their own subtree: scene/bank changes rerun just this
;; bar, not the whole transport. Nothing shows before the host has published
;; the banks.
(def transport-scene-strip ()
  (subtree :key "transport-pattern-pills"
    (let ((b (scene-viewed-bank)))
      (if b (scene-strip b) (nothing)))))

(def transport-trailing ()
  (list
    (subtree :key "transport-transpose-context-menu"
      (transpose-context-menu))
    (subtree :key "transport-context-menus"
      (h-stack :gap 0 (map application-context-menu (application-menu-names))))
    
    ;; Session and arrangement are app views, not tabs in the main buffer.
    ;; This spacer keeps the view pair against the transport's right edge.
    (box :width 0 :flex 1)
    (subtree :key "transport-session-view-button"
      (session-view-icon
        :on-click |x y r| (eseq.seq-panels/seq-show-sequencer-main)
        :style transport-icon-style
        :active (if (tabs/seq-arrangement-view?) 0 1)))
    (subtree :key "transport-arrangement-view-button"
      (arrangement-view-icon
        :on-click |x y r| (eseq.seq-panels/seq-open-arrangement)
        :style transport-icon-style
        :active (if (tabs/seq-arrangement-view?) 1 0)))
  ))

(def transport-body ()
  (h-stack :key "transport-bar" :width :fill :gap 0.5 :padding 0.5 :align :center
    (transport-leading)
    (transport-scene-strip)
    (transport-trailing)))

(effect-buffer "*transport*"
  (transport-body))
