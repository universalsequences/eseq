;; ui/transport.lisp — Transport bar UI (Logic Pro style)
;; Renders to *transport* buffer. Loaded by ui/main.lisp. Converted in S3b.
;;
;; NEVER `(import eseq.transport …)`. `import` EVALUATES its target and this
;; file is a live render root: the `(effect-buffer "*transport*" …)` at the
;; bottom registers a buffer at top level, so importing it would drag the
;; transport UI into every VM that loads the importer (the wave-2 lesson that
;; broke 60 tests). Callers reach the names below through the identity compat
;; aliases instead — which is also why `pattern-control-style`, used by the
;; converted `eseq.step-grid` and `eseq.sequencer`, is aliased rather than
;; requalified at those call sites.
;;
;; Exactly one `import` (below). Every OTHER outbound reference is left bare:
;; a Rust native (`seq-toggle-play`, `seq-set-bpm`, `seq-toggle-record`,
;; `seq-toggle-master-recording`, `seq-song-back-to-song`, `host-command`,
;; `bind-seq`), a function in ui/seq-panels.lisp (the panel toggles and the two
;; view switches), or a name owned by a UI ROOT module reached through that
;; root's own compat alias (`set-arrangement-cursor` → eseq.arrangement,
;; `sbrowser-*` → eseq.browser). All of those are reached only from `on-click`
;; lambdas — i.e. at event time, long after main.lisp has loaded everything —
;; so the stage-3 late-binding heal covers them even though main.lisp loads
;; this file (line 25) BEFORE seq-panels.lisp, sequencer.lisp and
;; arrangement.lisp. A reference evaluated during the RENDER cannot rely on
;; that; see the import note below.
;;
;; The three panel-visibility reads (`samples-sidebar-visible`,
;; `mixer-panel-visible`, `lower-panel-visible`) stay BARE. They are
;; `defstate`s owned by `eseq.seq-core-state`, which main.lisp loads first;
;; a bare read resolves through that module's identity alias into the *same*
;; state node (compiler.rs `state_binding_for`'s compat-alias rung), so the
;; reads stay reactive and hazard (m) does not apply.
;;
;; Hazard (n): no Rust harness slices this file's source — the single
;; `crates/sequencer/src` mention of "ui/transport.lisp" is the
;; `metal_seq_core_lisp_files_parse` file list, a whole-file parse — so the
;; `:as` alias below is safe (a fragment eval would need the full dotted
;; `eseq.seq-step-tabs/…` spelling instead).
(module eseq.transport)
;; Compile-time edge (spec §4): the shared defstate keyspace + compat
;; aliases must exist before this unit's readers compile.
(import eseq.seq-core-state)

;; The ONE import, and it is load-bearing rather than cosmetic. The two view
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

;; Shared scene-bank view state (see the module header). A state/accessor
;; hub with no effect-buffer, and ui/main.lisp reaches it through this import
;; before the transport body runs.
(import eseq.scene-banks)

(export transport-stop
        seq-set-scene-launch-quantize
        seq-set-record-quantize
        seq-switch-pattern
        seq-clone-pattern
        seq-delete-pattern
        seq-reorder-scene-drop
        scene-push-target
        scene-push-value
        scene-push-begin
        scene-push-drag
        scene-push-end
        pattern-control-style)

;; Identity compat aliases (spec §10 slice 3). Each covers a flat caller that
;; cannot see a qualified name; every one is a function or a `defstate`, both
;; immune to hazard (m):
;;   transport-stop, seq-set-scene-launch-quantize, seq-set-record-quantize,
;;   seq-switch-pattern, seq-reorder-scene-drop, scene-push-begin,
;;   scene-push-drag — evaluated by name from Rust tests in
;;   src/ui/state_values/tests.rs.
;;   scene-push-target, scene-push-value — `defstate`s that flat callers WRITE:
;;   ui/capture-fixtures/scene-push-transport.lisp and the
;;   `(set! scene-push-target 1)` eval in state_values/tests.rs. The alias
;;   covers the `state_bindings` keyspace too, so those writes still land on
;;   this module's state node (no eseq.vanilla pin needed).
;;   pattern-control-style — a write-once style `def` read bare by
;;   ui/step-grid.lisp (eseq.step-grid) and ui/sequencer.lisp (eseq.sequencer).
;;   Neither may import a UI root, so the alias is the supported edge.

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
    (sdf/fill (sdf/rounded-rect width height 0.7)
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
  :bindable (level)
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
  :bindable (active)
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
  (do
    (if SEQ.playing (seq-toggle-play) nil)
    (eseq.arrangement/set-cursor 0 -1)))

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

(def scene-launch-quantize-options '("off" "1/16" "1/8" "1/4" "1/2" "1 bar"))

(def record-quantize-options '("off" "1/16" "1/8" "1/4" "1/2" "1 bar"))

(def seq-set-scene-launch-quantize (value)
  (host-command "set-scene-launch-quantize" value))

(def seq-set-record-quantize (value)
  (host-command "set-record-quantize" value))

(def seq-switch-pattern (idx)
  (host-command "switch-pattern"
    (dict :idx idx :quantize (or SEQ.scene-launch-quantize "off"))))

;; Scene-bank view state lives in eseq.scene-banks so the mixer clip grid
;; (scene-banks spec 10.1) shares one viewed bank with this strip. These four
;; are thin local spellings of that module's accessors; the writes below name
;; the module's state directly.
(def scene-banks ()
  (eseq.scene-banks/scene-banks))

(def scene-bank-index-containing (scene)
  (eseq.scene-banks/scene-bank-index-containing scene))

(def scene-viewed-bank-index ()
  (eseq.scene-banks/scene-viewed-bank-index))

(def scene-viewed-bank ()
  (eseq.scene-banks/scene-viewed-bank))

(def scene-bank-labels ()
  (reduce |labels bank|
    (append labels (list (get bank :label)))
    (list)
    (scene-banks)))

(def scene-bank-index-for-label (label)
  (let ((banks (scene-banks)))
    (let ((matches (filter
            (lambda (i) (= (get (nth banks i) :label) label))
            (range 0 (len banks)))))
      (if (> (len matches) 0) (nth matches 0) -1))))

(def select-scene-bank (label)
  (if (= label "New bank")
    (do
      ;; create-scene-bank appends. Keep the pending index unclamped until the
      ;; host publishes the new SEQ.scene-banks entry, then the view lands on it.
      (set! eseq.scene-banks/viewed-scene-bank-index (len (scene-banks)))
      (set! eseq.scene-banks/viewed-scene-bank-pending-new true)
      (host-command "create-scene-bank" (dict)))
    (let ((index (scene-bank-index-for-label label)))
      (if (>= index 0)
        (do
          (set! eseq.scene-banks/viewed-scene-bank-pending-new false)
          (set! eseq.scene-banks/viewed-scene-bank-index index))
        nil))))

(def scene-playing-in-other-bank? ()
  (not (= (scene-bank-index-containing SEQ.current-pattern)
    (scene-viewed-bank-index))))

(defstate scene-bank-ops-menu-open false)
(defstate scene-bank-ops-menu-col 0)
(defstate scene-bank-ops-menu-row 0)
(defstate scene-bank-renaming false)
(defstate scene-bank-rename-id 0)
(defstate scene-bank-rename-draft "")

(def open-scene-bank-ops-menu (event)
  (do
    (set! scene-bank-ops-menu-col (get event :col))
    (set! scene-bank-ops-menu-row (get event :row))
    (set! scene-bank-ops-menu-open true)))

(def begin-scene-bank-rename ()
  (let ((bank (scene-viewed-bank)))
    (do
      (set! scene-bank-ops-menu-open false)
      (set! scene-bank-rename-id (get bank :id))
      (set! scene-bank-rename-draft (or (get bank :name) ""))
      (set! scene-bank-renaming true))))

(def scene-bank-rename-changed? ()
  (let ((matches (filter
          (lambda (bank) (= (get bank :id) scene-bank-rename-id))
          (scene-banks))))
    (if (> (len matches) 0)
      ;; Stored names are trimmed host-side, so compare against the trimmed
      ;; draft; a no-op commit would otherwise surface a host error status.
      (not (= (string-trim scene-bank-rename-draft)
          (or (get (nth matches 0) :name) "")))
      false)))

(def finish-scene-bank-rename (commit)
  (if scene-bank-renaming
    (do
      (if (and commit (scene-bank-rename-changed?))
        (host-command "rename-scene-bank"
          (dict :bank-id scene-bank-rename-id :name scene-bank-rename-draft))
        nil)
      (set! scene-bank-renaming false)
      (set! scene-bank-rename-id 0)
      (set! scene-bank-rename-draft ""))
    nil))

(def scene-viewed-bank-deletable? ()
  (let ((banks (scene-banks))
        (index (scene-viewed-bank-index)))
    (if (<= (len banks) 1)
      false
      (let ((target-index (if (= index 0) 1 (- index 1))))
        (<= (+ (get (nth banks index) :len)
            (get (nth banks target-index) :len))
          24)))))

(def delete-viewed-scene-bank ()
  (if (scene-viewed-bank-deletable?)
    (let ((bank (scene-viewed-bank))
          (fallback-index (max 0 (- (scene-viewed-bank-index) 1))))
      (do
        (set! scene-bank-ops-menu-open false)
        (set! eseq.scene-banks/viewed-scene-bank-pending-new false)
        (set! eseq.scene-banks/viewed-scene-bank-index fallback-index)
        (host-command "delete-scene-bank" (dict :bank-id (get bank :id)))))
    nil))

(def scene-bank-ops-context-menu ()
  (context-menu :is-open scene-bank-ops-menu-open
    :anchor-col scene-bank-ops-menu-col
    :anchor-row scene-bank-ops-menu-row
    :on-close (lambda () (set! scene-bank-ops-menu-open false))
    (menu-item "Rename bank"
      :key "scene-bank-rename-action"
      :on-select (lambda (event) (begin-scene-bank-rename)))
    (menu-item "Delete bank"
      :key "scene-bank-delete-action"
      :disabled (not (scene-viewed-bank-deletable?))
      :on-select (lambda (event) (delete-viewed-scene-bank)))))

(def scene-bank-selector (bank)
  (box :key "scene-bank-selector"
    :width 4.2 :height 0.8
    :on-right-click (lambda (event) (open-scene-bank-ops-menu event))
    (if scene-bank-renaming
      (text-input :key "scene-bank-rename-input"
        :width 4.2 :height 0.8 :font-size 8
        :value scene-bank-rename-draft
        :auto-focus true
        :select-all-on-focus true
        :on-change (lambda (name) (set! scene-bank-rename-draft name))
        :on-submit (lambda () (finish-scene-bank-rename true))
        :on-cancel (lambda () (finish-scene-bank-rename false))
        :on-blur (lambda () (finish-scene-bank-rename true)))
      (dropdown :key "scene-bank-dropdown"
        :value (get bank :label)
        :options (append (scene-bank-labels) (list "New bank"))
        :on-change select-scene-bank
        :bg-color :mixer-strip-bg
        :border-color :mixer-strip-border
        :badge-color :transparent
        :width 4.2 :height 0.5 :font-size 10))))

(def seq-clone-pattern ()
  (let ((bank (scene-viewed-bank)))
    (host-command "clone-pattern"
      (dict :bank-id (get bank :id)
        :insert-position (+ (get bank :offset) (get bank :len))))))

(def seq-delete-pattern ()
  (host-command "delete-pattern" (dict)))

(defstate scene-bank-menu-open false)
(defstate scene-bank-menu-col 0)
(defstate scene-bank-menu-row 0)
(defstate scene-bank-menu-scene -1)

(def open-scene-bank-menu (event scene)
  (do
    (set! scene-bank-menu-scene scene)
    (set! scene-bank-menu-col (get event :col))
    (set! scene-bank-menu-row (get event :row))
    (set! scene-bank-menu-open true)))

(def scene-bank-is-source? (bank)
  (= (scene-bank-index-containing scene-bank-menu-scene)
    (scene-bank-index-for-label (get bank :label))))

(def move-scene-to-scene-bank (bank)
  (if (or (scene-bank-is-source? bank) (>= (get bank :len) 24))
    nil
    (do
      (set! scene-bank-menu-open false)
      (host-command "move-scene-to-scene-bank"
        (dict :scene scene-bank-menu-scene :bank-id (get bank :id))))))

(def scene-bank-context-menu ()
  (context-menu :is-open scene-bank-menu-open
    :anchor-col scene-bank-menu-col
    :anchor-row scene-bank-menu-row
    :on-close (lambda () (set! scene-bank-menu-open false))
    (each (scene-banks) |bank|
      (menu-item (str "Move to bank " (get bank :label))
        :key (str "scene-bank-move-" (get bank :id))
        :disabled (or (scene-bank-is-source? bank) (>= (get bank :len) 24))
        :on-select (lambda (event) (move-scene-to-scene-bank bank))))))

(def seq-reorder-scene-drop (event)
  (let ((source (get (get event :payload) :scene))
        (target (get (get event :target) :scene)))
    (if (= source target)
      nil
      (host-command "reorder-scene" (dict :source source :target target)))))

;; Shift gesture state is UI-local and intentionally ephemeral. The modifier
;; is sampled only on pointer-down; releasing Shift while still holding the
;; mouse cannot turn the gesture into a reorder operation.
(def scene-push-target (state -1))
(def scene-push-value (state 1.0))
(def scene-push-start-y (state 0.0))
(def scene-push-from-source (state false))

(def scene-push-begin (scene event)
  (let ((from-source (or (get event :cmd) (get event :meta) (get event :super))))
  (if (or (get event :shift) from-source)
    (do
      (set! scene-push-target scene)
      (set! scene-push-from-source from-source)
      (set! scene-push-value (if from-source 0.0 1.0))
      (set! scene-push-start-y (get event :y))
      (host-command "scene-push-begin"
        (dict :target-scene scene :value (if from-source 0.0 1.0))))
    (seq-switch-pattern scene))))

(def scene-push-drag (scene event)
  (if (= scene-push-target scene)
    (let ((value (if scene-push-from-source
          (clamp (* 0.14 (- (get event :y) scene-push-start-y)) 0.0 1.0)
          (clamp (+ 1.0 (* 0.14 (- scene-push-start-y (get event :y)))) 0.0 1.0))))
      (set! scene-push-value value)
      (host-command "scene-push-set-value" (dict :value value)))
    nil))

(def scene-push-end (scene event)
  (if (= scene-push-target scene)
    (do
      (host-command "scene-push-end" (dict))
      (set! scene-push-target -1)
      (set! scene-push-from-source false)
      (set! scene-push-value 1.0))
    nil))

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

(defstate transpose-menu-open false)
(defstate transpose-menu-col 0)
(defstate transpose-menu-row 0)
(defstate transpose-menu-value 0)
(defstate transpose-menu-bank 0)

(def open-transpose-menu (event)
  (do
    (set! transpose-menu-value scene-transpose)
    (set! transpose-menu-bank (get (scene-viewed-bank) :id))
    (set! transpose-menu-col (get event :col))
    (set! transpose-menu-row (get event :row))
    (set! transpose-menu-open true)))

(def apply-transpose-menu (scope)
  (do
    (set! transpose-menu-open false)
    (host-command "apply-scene-transpose"
      (dict :scope scope :bank-id transpose-menu-bank :value transpose-menu-value))))

(def transpose-context-menu ()
  (context-menu :is-open transpose-menu-open
    :anchor-col transpose-menu-col :anchor-row transpose-menu-row
    :on-close (lambda () (set! transpose-menu-open false))
    (menu-item "Apply to all scenes in this bank"
      :key "transpose-apply-bank"
      :on-select (lambda (event) (apply-transpose-menu "bank")))
    (menu-item "Apply to all scenes in all banks"
      :key "transpose-apply-all-banks"
      :on-select (lambda (event) (apply-transpose-menu "all-banks")))))

;; ── Transport layout ──

(effect-buffer "*transport*"
  (h-stack :width :fill :gap 0.5 :padding 0.5 :align :center
    
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
    (subtree :key "transport-save-button"
      (save-icon
        :on-click |x y r| (eseq.browser/open-project-save)
        :style transport-icon-style
        :active (if (eseq.browser/project-save-mode?) 1 0)))
    
    ;; Transport buttons in a shared rounded-rect container
    (box :background-color :mixer-strip-bg :corner-radius 72 :padding 0.015 :height 1.4
      (h-stack :gap 0.2 :align :center
        (subtree :key "transport-stop-button"
          (box :width 2.5
            :on-click |x y r| (transport-stop)
            (stop-icon)))
        (box :width 2.5
          :on-click |x y r| (seq-toggle-play)
          (play-icon :active (if SEQ.playing 1 0)))
        (box :width 2.5
          :on-click |x y r| (seq-toggle-record)
          (rec-icon :active (if SEQ.recording 1 0)))
        (subtree :key "transport-master-record-button"
          (box :debug-name "transport-master-record-button"
            :width 4.2 :height 1.1
            :background "pattern-pill-bg"
            :active (if SEQ.master-recording 1 0)
            :style transport-icon-style
            :on-click |x y r| (seq-toggle-master-recording)
            (v-stack :align :center
              (label "WAV"
                :font-size 10
                :color (if SEQ.master-recording :white :gray)
                :hover-color :white
                :bg :transparent))))
        ;; Back to Arrangement (unified-transport spec; Ableton semantics):
        ;; lights the moment a manual launch overrides the arrangement,
        ;; SURVIVES transport stop, and clicking hands the latched lanes
        ;; back to the arrangement. The box is ALWAYS laid out (state flips
        ;; repaint reactively; conditional layout is a re-layout per flip
        ;; and misses reruns from nil); the icon is transparent while
        ;; nothing is latched, and the click guards at event time.
        (subtree :key "transport-back-to-arrangement"
          (box :debug-name "transport-back-to-arrangement"
            :width 2.5 :height 1.4
            :style transport-icon-style
            :on-click |x y r| (if SEQ.song-manual-latch (seq-song-back-to-song) nil)
            (back-to-arrangement-icon
              :active (if SEQ.song-manual-latch 1 0))))))
    
    ;; Single continuous LED panel
    (box :background-color :mixer-strip-bg :corner-radius 64 :height 1.4 :width 77
      (h-stack
        (subtree :key "transport-clock"
          (h-stack :gap 0 :align :center :padding 0.5
            (transport-clock
              ;; One transport (docs/unified-transport-spec.md 4/8): the
              ;; parked arrangement cursor while stopped, the live absolute
              ;; arrangement clock during playback/capture.
              :playhead (bind-seq "transport-playhead")
              :song-position-beats
              (if (= SEQ.song-mode "stopped")
                (bind-seq "song-cursor-beats")
                (bind-seq "song-position-beats"))
              :use-song-position true
              :font-size 15 :width 10 :height 1.2
              :color :clock-fg
              :bg :transparent)
            (label "" :width 1 :bg :transparent)
            (number-picker :value SEQ.bpm :min 20 :max 300 :decimals 1
              :key "transport-bpm"
              :noui true
              :font-size 15
              :text-color :clock-fg
              :on-change (lambda (v) (seq-set-bpm v))
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
                :value (or SEQ.scene-launch-quantize "off")
                :options scene-launch-quantize-options
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
                :value (or SEQ.record-quantize "1/16")
                :options record-quantize-options
                :on-change seq-set-record-quantize
                :width 5.2 :height 1.15 :font-size 9))
            (box :width 1.5)
            (subtree :key "transport-metronome-toggle"
              (box :debug-name "transport-metronome-toggle"
                :width 3.4 :height 1.1
                :background "pattern-pill-bg"
                :on-click |x y r| (host-command "toggle-metronome")
                (v-stack :align :center
                  (label "MET"
                    :font-size 9
                    :color (if SEQ.metronome :white :gray)
                    :hover-color :white
                    :bg :transparent))))
            ;; Roll mode (docs/rolling-core-spec.md 8): toggle + live rate
            ;; display. Rate keys 1-8 switch the rate while roll mode is on.
            (subtree :key "transport-roll-toggle"
              (box :debug-name "transport-roll-toggle"
                :width 5.5 :height 1.1
                :background-color (if SEQ.sequence-rolling
                  '(rgba 0.72 0.10 0.12 1)
                  "pattern-pill-bg")
                :on-click |x y r| (host-command "toggle-roll-mode")
                (h-stack :align :baseline :gap 0.3
                  (box :width 0.2)
                  (label "ROLL"
                    :font-size 9
                    :color (if SEQ.roll-mode :white :gray)
                    :hover-color :white
                    :bg :transparent)
                  (label (if SEQ.roll-mode SEQ.roll-rate "")
                    :font-size 9
                    :color '(rgba 0.63 0.88 0.41 1)
                    :bg :transparent))))))
        (v-stack :gap 0.08 :padding 0.05
          (label "L"
            :font-size 5 :width 0.9
            :color '(rgba 0.63 0.88 0.41 1)
            :bg :transparent)
          
          (label "R"
            :font-size 5 :width 0.9
            :color '(rgba 0.63 0.88 0.41 1)
            :bg :transparent)          )
        
        
        (v-stack :gap 0.08 :padding 0.05
          (h-stack :gap 0.25
            
            (v-stack
              (box :height 0.2)
              (subtree :key "master-meter-l"
                (transport-master-meter :level (bind-seq "master-peak-l")))))
          (h-stack :gap 0.25 :align :center
            
            (v-stack (box :height 0.1)
              (subtree :key "master-meter-r"
                (transport-master-meter :level (bind-seq "master-peak-r"))))))
        (subtree :key "transport-cpu"
          (h-stack :gap 0 :align :center :padding 0.4
            (box :height 2.7
              (label "cpu"
                :font-size 12 :width 3.0
                :color :gray
                :bg :transparent))
            (number-label :value (bind-seq "cpu-load-pct")
              :decimals 0 :min-integer-digits 2 :suffix "%"
              :font-size 12 :width 2.0 :height 1
              :color :dim
              :bg :transparent)))
        ;; The latency planner aligns every route to this delay. A latent FX
        ;; therefore delays the whole project, not only the track holding it.
        (subtree :key "transport-output-latency"
          (h-stack :gap 0 :align :baseline :padding 0.5
            (number-label :key "transport-output-latency-value"
              :value (bind-seq "output-latency-ms")
              :decimals 1 :min-integer-digits 2 :suffix "ms"
              :font-size 12 :width 3.5 :height 1
              :color :gray
              :bg :transparent)))))
    
    ;; Pattern pills in their own subtree: scene/bank changes rerun just this
    ;; bar, not the whole transport. Widget children stay in `each`; the bank
    ;; offset is applied before every launch, drag, and context-menu command.
    (subtree :key "transport-pattern-pills"
      (let ((bank (scene-viewed-bank)))
        (let ((bank-offset (get bank :offset))
            (bank-len (get bank :len))
            (current-in-bank (= (scene-bank-index-containing SEQ.current-pattern)
                (scene-viewed-bank-index))))
          (box :background "transport-scene-strip-bg"
            :corner-radius 64
            :key "transport-scene-strip"
            :debug-name "transport-scene-strip"
            :push scene-push-value
            :push-target (if (and (>= scene-push-target bank-offset)
                (< scene-push-target (+ bank-offset bank-len)))
              (- scene-push-target bank-offset)
              -1)
            :scene-count bank-len
            :padding 0.2 :height 1.4
            (h-stack :gap 0.1 :align :center
              (each (range 0 bank-len) |i|
                (let ((scene (+ bank-offset i)))
                  (box :key (str "transport-scene-pill-" scene)
                    :width 2.5 :height 1.1
                    :background (if (= scene SEQ.queued-scene)
                      "queued-scene-pill-bg"
                      "pattern-pill-bg")
                    :active (if (= scene SEQ.current-pattern) 1 0)
                    :push scene-push-value
                    :push-target scene-push-target
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
                    :on-right-click (lambda (event) (open-scene-bank-menu event scene))
                    :on-mouse-down (lambda (event) (scene-push-begin scene event))
                    :on-drag (lambda (event) (scene-push-drag scene event))
                    :on-mouse-up (lambda (event) (scene-push-end scene event))
                    (v-stack :align :center
                      (label (fmt " {} " (+ i 1))
                        :font-size 11
                        :color (if (or (= scene SEQ.current-pattern) (= scene SEQ.queued-scene))
                          :scene-active-fg
                          :gray)
                        :hover-color :white
                        :bg :transparent)))))
              (label "" :width 0.2 :bg :transparent)
              (box :key "scene-bank-add" :background "pattern-pill-btn-bg"
                :width 2.5 :height 1.1 :active true
                :style (if (< bank-len 24) pattern-control-style nil)
                :on-click |x y r|
                (if (< bank-len 24)
                  (seq-clone-pattern)
                  (status "This scene bank is full (24 scenes maximum)"))
                (v-stack :align :center
                  (label "+"
                    :font-size 12
                    :color (if (< bank-len 24) :white :dark-gray)
                    :bg :transparent)))
              (box :key "scene-bank-delete" :background "pattern-pill-btn-bg"
                :width 2.5 :height 1.1 :active true
                :style (if (and (> SEQ.num-patterns 1) current-in-bank)
                  pattern-control-style
                  nil)
                :on-click |x y r|
                (if (and (> SEQ.num-patterns 1) current-in-bank)
                  (seq-delete-pattern)
                  nil)
                (v-stack :align :center
                  (label "-"
                    :font-size 12
                    :color (if (and (> SEQ.num-patterns 1) current-in-bank)
                      :white
                      :dark-gray)
                    :bg :transparent)))
              (h-stack :gap 0.12 :align :center
                (box :width 0.5)
                (scene-bank-selector bank)
                (if (scene-playing-in-other-bank?)
                  (scene-bank-playing-indicator
                    :debug-name "scene-bank-playing-other-indicator")
                  (box :width 0.45 :height 0.45 :bg :transparent)))
              (scene-bank-context-menu)
              (scene-bank-ops-context-menu))))))
    
    (subtree :key "transport-transpose-context-menu"
      (transpose-context-menu))

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
        :active (if (tabs/seq-arrangement-view?) 1 0)))))
