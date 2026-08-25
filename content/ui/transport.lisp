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
        ;; and 0.1 gaps. The trailing spacer/+/- controls account for 5.4 cells.
        (total-cells (+ (* count 2.6) 5.8))
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
        (material :color (rgba 0.18 0.18 0.20 1.0)
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
            (let ((base (rgba 0.00 0.01 0.42 1.0))
                  (lit (+ 0.06 (* 0.03 diffuse)))
                  (shine (* 0.25 specular)))
              (+ base (rgba lit lit lit 1) (rgba shine shine shine 0)))
            (if hit/hover
              (let ((base (rgba 0.10 0.10 0.12 0.72))
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
              (rgba
                (+ 0.04 lit shine)
                (+ 0.24 lit shine)
                (+ 0.88 lit shine)
                (* 0.88 amount)))))
        (rgba 0 0 0 0)))))

(defwidget queued-scene-pill-bg
  :width 1 :height 1
  :state (push push-target scene)
  :paint-margin 0.3
  :animates true
  :shader
  (let ((pulse (+ 0.5 (* 0.5 (cos (* itime 5.4)))))
        (push-amount (if (= scene push-target) push 0.0))
        (base (rgba
          (+ 0.01 (* 0.04 pulse))
          (+ 0.03 (* 0.10 pulse))
          (+ 0.38 (* 0.30 pulse))
          1.0)))
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
              (rgba
                (+ 0.04 lit shine)
                (+ 0.24 lit shine)
                (+ 0.88 lit shine)
                (* 0.72 amount)))))
        (rgba 0 0 0 0)))))


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
          (let ((base (rgba 0.00 0.01 0.02 1.0))
                (lit (+ 0.06 (* 0.03 diffuse)))
                (shine (* 0.25 specular)))
            (+ base (rgba lit lit lit 1) (rgba shine shine shine 0)))
          (if hit/hover
            (let ((base (rgba 0.10 0.10 0.12 0.72))
                  (lit (+ 0.06 (* 0.03 diffuse)))
                  (shine (* 0.25 specular)))
              (+ base (rgba lit lit lit 1) (rgba shine shine shine 0)))
            (rgba 0 0 0 0)))))))

(defwidget add-track-icon
  :width 2.5 :height 2.5
  :paint-margin 0.5
  :state (active)
  :shader
  (let ((fg-col (if (= active 1) (rgba 1 1 1 1.0) (rgba 0.75 0.75 0.78 1.0))))
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
  (let ((fg-col (rgba 0.92 0.92 0.96 1.0))
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
      (muted-col (rgba 0.25 0.25 0.20 1.0))
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
      (muted-col (rgba 0.25 0.25 0.20 1.0))
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
      (muted-col (rgba 0.25 0.25 0.20 1.0))
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
      (muted-col (rgba 0.25 0.25 0.27 1.0))
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
      (muted-col (rgba 0.25 0.25 0.27 1.0))
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
      (material :color (rgba 0.75 0.75 0.78 1.0)))))

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
  (let ((fg-col (if (= active 1) (rgba 1 1 1 1.0) (rgba 0.75 0.75 0.78 1.0))))
    (sdf/layer
      (if (= active 1)
        (sdf/fill (sdf/rounded-rect (* 0.75 height) (* 0.75 height) 0.4)
          (material
            :lighting (lighting :edge-min -0.1015 :edge-max 0.9413
              :light (vec3 -0.31 -0.851 1.3) :shininess 51.0)
            :color
            (let ((base (rgba 0.05 0.28 0.03 1.0))
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

;; Back to Arrangement (Ableton-style): an orange tile with a play triangle
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
            (+ (rgba 0.80 0.38 0.16 1.0)
               (rgba lit (* 0.6 lit) (* 0.3 lit) 1)
               (rgba shine shine shine 0)))))
      (sdf/fill
        (let ((p1x -0.62) (p1y -0.34) (p2x -0.62) (p2y 0.34) (p3x -0.10) (p3y 0.0))
          (let ((d1 (- (* (- p2x p1x) (- y p1y)) (* (- p2y p1y) (- x p1x))))
                (d2 (- (* (- p3x p2x) (- y p2y)) (* (- p3y p2y) (- x p2x))))
                (d3 (- (* (- p1x p3x) (- y p3y)) (* (- p1y p3y) (- x p3x)))))
            (max (max d1 d2) d3)))
        (material :color (rgba 0.10 0.045 0.02 1.0)))
      (sdf/fill (sdf/translate 0.38 -0.28 (sdf/rounded-rect 0.28 0.055 0.03))
        (material :color (rgba 0.10 0.045 0.02 1.0)))
      (sdf/fill (sdf/translate 0.38 0.0 (sdf/rounded-rect 0.28 0.055 0.03))
        (material :color (rgba 0.10 0.045 0.02 1.0)))
      (sdf/fill (sdf/translate 0.38 0.28 (sdf/rounded-rect 0.28 0.055 0.03))
        (material :color (rgba 0.10 0.045 0.02 1.0))))
    (rgba 0 0 0 0)))

(defwidget rec-icon
  :width 2.5 :height 1.8
  :paint-margin 0.5
  :state (active)
  :shader
  (let ((fg-col (if (= active 1) (rgba 1 1 1 1.0) (rgba 0.65 0.18 0.18 1.0))))
    (sdf/layer
      (if (= active 1)
        (sdf/fill (sdf/rounded-rect (* 0.75 height) (* 0.75 height) 0.4)
          (material
            :lighting (lighting :edge-min -0.1015 :edge-max 0.9413
              :light (vec3 -0.31 -0.851 1.5) :shininess 51.0)
            :color
            (let ((base (rgba 0.12 0.001 0.001 1.0))
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

(def seq-clone-pattern ()
  (host-command "clone-pattern" (dict)))

(def seq-delete-pattern ()
  (host-command "delete-pattern" (dict)))

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
    (box :background-color :mixer-strip-bg :corner-radius 64 :height 1.4 :width 68
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
              :color '(rgba 0.85 0.85 0.85 1)
              :bg :transparent)
            (label "" :width 1 :bg :transparent)
            (number-picker :value SEQ.bpm :min 20 :max 300 :decimals 1
              :noui true
              :font-size 15
              :text-color (rgba 0.85 0.85 0.85 1)
              :on-change (lambda (v) (seq-set-bpm v))
              :width 7 :height 1.2)
            (subtree :key "transport-scene-launch-quantize"
              (dropdown
                :bg-color '(rgba 0.1 0.1 0.1 0.3) ;:instrument-control-bg
                :border-color '(rgba 0.4 0.4 0.4 1)
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
                :bg-color '(rgba 0.1 0.1 0.1 0.3)
                :border-color '(rgba 0.4 0.4 0.4 1)
                :badge-color :transparent
                :key "transport-record-quantize-dropdown"
                :debug-name "transport-record-quantize"
                :value (or SEQ.record-quantize "1/16")
                :options record-quantize-options
                :on-change seq-set-record-quantize
                :width 5.2 :height 1.15 :font-size 9))
            (box :width 0.5)
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
                :color '(rgba 0.30 0.30 0.32 1)
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
    
    ;; Pattern pills in their own subtree: current-pattern/num-patterns changes
    ;; (every scene switch) rerun just this bar, not the whole transport.
    (subtree :key "transport-pattern-pills"
      (box :background-color :mixer-strip-bg
        :corner-radius 64
        :key "transport-scene-strip"
        :debug-name "transport-scene-strip"
        :push scene-push-value
        :push-target scene-push-target
        :scene-count SEQ.num-patterns
        :padding 0.2 :height 1.4
        (h-stack :gap 0.1 :align :center
          (each (range 0 SEQ.num-patterns) |i|
            (box :key (str "transport-scene-pill-" i)
              :width 2.5 :height 1.1
              :background (if (= i SEQ.queued-scene)
                "queued-scene-pill-bg"
                "pattern-pill-bg")
              :active (if (= i SEQ.current-pattern) 1 0)
              :push scene-push-value
              :push-target scene-push-target
              :scene i
              :style pattern-control-style
              :capture-pointer true
              :drag-type "transport-scene"
              :drag-modifier :none
              :drag-payload (dict :scene i)
              :drop-types (list "transport-scene")
              :drop-meta (dict :scene i)
              :drop-hover-border-color :mixer-strip-selected-border
              :on-drop seq-reorder-scene-drop
              :on-mouse-down (lambda (event) (scene-push-begin i event))
              :on-drag (lambda (event) (scene-push-drag i event))
              :on-mouse-up (lambda (event) (scene-push-end i event))
              (v-stack :align :center
                (label (fmt " {} " (+ i 1))
                  :font-size 11
                  :color (if (or (= i SEQ.current-pattern) (= i SEQ.queued-scene))
                    :white
                    :gray)
                  :hover-color :white
                  :bg :transparent))))
          (label "" :width 0.2 :bg :transparent)
          (box :background "pattern-pill-btn-bg" :width 2.5 :height 1.1 :active true
            :style pattern-control-style
            :on-click |x y r| (seq-clone-pattern)
            (v-stack :align :center
              (label "+"
                :font-size 12
                
                :color :white
                :bg :transparent)))
          
          (box :background "pattern-pill-btn-bg" :width 2.5 :height 1.1 :active true
            :style (if (> SEQ.num-patterns 1) pattern-control-style nil)
            :on-click |x y r| (if (> SEQ.num-patterns 1) (seq-delete-pattern) nil)
            (v-stack :align :center
              (label "-"
                :font-size 12
                
                :color (if (> SEQ.num-patterns 1) :white :dark-gray)
                :bg :transparent))))))
    
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
