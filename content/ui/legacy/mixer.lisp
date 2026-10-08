;; ui/legacy/mixer.lisp - retained predecessor to ui/mixer.lisp; not loaded by
;; ui/main.lisp. Its top-level structure and direct loading have a dedicated
;; metal_seq regression test. Renders to the *mixer* buffer when evaluated.
;;
;; Converted in S3b wave 10. Notes specific to this file:
;;
;;   * Dead reference code: a whole-tree sweep of crates/sequencer/src,
;;     crates/eseqlisp/src and content/ui found ZERO callers of any
;;     name defined here, and no loader/harness that reads this path. So every
;;     `def` below is unexported and there are no compat aliases at all.
;;     The live mixer (`ui/mixer.lisp` = `eseq.mixer`) owns the public
;;     mixer-shaped names; privatizing here keeps the two from ever competing
;;     for a flat slot (spec §10 hazard k).
;;   * The four `defwidget`s keep their flat, unrenamed names (hazard e —
;;     `defwidget` is its own flat keyspace). None of `track-container`,
;;     `rec-arm-dot`, `mixer-track-meter`, `delete-track-icon` is defined by
;;     any other ui lisp file; ui/sequencer.lisp's lookalikes are the distinct
;;     `seqv-`-prefixed pair. No new clash is introduced.
;;   * The `:shader` bodies expand outside this module in a throwaway
;;     implicit-module compiler (hazard g/h). Material helpers therefore use
;;     their explicit `eseq.materials/` names rather than module imports.
;;   * Bus selection reads and writes use the explicit
;;     `eseq.seq-core-state/selected-bus` state name.
;;   * Host state comes from eseq.kinds (docs/kind-bindings-spec.md): tracks
;;     and buses are instances, meters, faders and mute/solo states are `#'`
;;     bindings. eseq.materials is imported so `rec-arm-dot`'s shader
;;     (`eseq.materials/color`) and the sliders' material resolve when this
;;     file loads on its own.
;;   * `bus-row-label` and `display-buses` duplicate eseq.mixer's on purpose:
;;     importing eseq.mixer would evaluate the live mixer (its `*mixer*`
;;     effect-buffer and keymap) just to share two small functions.
;;   * Widget `:key` props auto-qualify, so the hand-rolled `mixer-` prefix is
;;     stripped from them; `(subtree :key …)` strings are left byte-identical
;;     (hazard a), as is the `"*mixer*"` buffer name.

(module eseq.legacy.mixer)
(import eseq.materials)
(import eseq.kinds :refer (tracks buses selection))

(export)

(defwidget track-container
  :width 1.5 :height 1.5
  :state (even selected)
  :shader
  (sdf/layer 
    (sdf/fill (sdf/rounded-rect width height 0.6) 
      (mix 
        (if selected (if even (rgba 1 1 1 1 ) (rgba 0.7 0.7 0.7 1)) (rgba 0 0 0 0))
        (if even (rgba 0 0 0 0) (rgba 0.1 0.1 0.1 1))
        (if selected (smoothstep 0 -0.1 d) 1)
        )
      )
    ))

;; Record arm indicator (small circle)
(defwidget rec-arm-dot
  :width 1.5 :height 1.5
  :state (active)
  :shader
  (sdf/layer
    (sdf/fill (sdf/circle 0.8)
      (material
        :lighting (lighting :edge-min -0.35 :edge-max 0.5
          :light (vec3 0.0 -1.0 1.5) :shininess 82.0)
        :color 
        (* (if (= active 1) 1.0 (+ 0.2 (smoothstep -0.4 0.1 d)))
          (eseq.materials/color
            (rgba 
              (if (= active 1) 0.85 0.5)
              (if (= active 1) 0.05 0.5) 
              (if (= active 1) 0.05 0.5) 
              1.0) 
            (rgba 0.99 0.15 0.15 1.0))
          )
        
        ))))

(def mute-button-bg (active)
  (if active
    (rgba 0.08 0.09 0.10 1.0)
    (rgba 0.115 0.130 0.144 1.0)))

(def solo-button-bg (active)
  (if active
    (rgba 0.72 0.10 0.10 1.0)
    (rgba 0.08 0.09 0.10 1.0)))

(def button-border (active)
  (if active
    (rgba 0.58 0.62 0.78 1.0)
    (rgba 0.28 0.29 0.32 1.0)))

(defwidget mixer-track-meter
  :width 5 :height 0.28
  :paint-margin 0.08
  :state (level)
  :shader
  (let ((lvl (min 1.0 (max 0.0 level)))
        (track (sdf/rounded-rect width height height))
        (green-end (min lvl 0.60))
        (yellow-end (min lvl 0.85))
        (red-end lvl))
    (sdf/layer
      (sdf/fill track
        (material :color (rgba 0.05 0.06 0.07 1)))
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
        (rgba 0 0 0 0)))))

(defwidget delete-track-icon
  :width 1.5 :height 1.5
  :paint-margin 0.35
  :state (active)
  :shader
  (let ((fg-col (if (= active 1)
                  (rgba 0.98 0.98 1.0 1.0)
                  (rgba 0.62 0.64 0.70 1.0)))
        (bg-col (if (= active 1)
                  (rgba 0.72 0.16 0.16 1.0)
                  (rgba 0.14 0.15 0.17 1.0))))
    (sdf/layer
      (sdf/fill (sdf/rounded-rect (* 1 width) (* 0.6 height) 1)
        (material :color bg-col))
      (sdf/fill
        (let ((clip (max (- (abs x) 0.28) (- (abs y) 0.28)))
              (diag1 (max (- (* 0.7071 (abs (- x y))) 0.045) clip))
              (diag2 (max (- (* 0.7071 (abs (+ x y))) 0.045) clip)))
          (min diag1 diag2))
        (material :color fg-col)))))

(def bus-row-label (b)
  (match b.index
    0 "M"
    1 "A"
    2 "B"
    _ (str b.index)))

;; The buses in display order: the main mix (first in the bus list) last.
(def display-buses ()
  (let ((all (buses))
        (mix (first all)))
    (if (and (> (len all) 1) (= mix.name "Mix"))
      (append (rest all) (list (first all)))
      all)))

(def current? (t)
  (and (< eseq.seq-core-state/selected-bus 0) (= selection.track t)))

(effect-buffer "*mixer*"
  (v-stack :padding 0.5 :gap 0.25
    (each (tracks) |t|
      (subtree :key (str "mixer-track-row-" t.index)
        (box :background "track-container"
          :padding 0.5
          :even (mod t.index 2)
          :selected (if (current? t) 1 0)
          (h-stack :gap 0.5 :align :center
            (box :width 2 :height 1.5
              :background "rec-arm-dot"
              :key (str "track-arm-" t.index)
              :active (if t.armed 1 0)
              :on-click |x y r| (do (set! eseq.seq-core-state/selected-bus -1) (toggle! t.armed)))
            (button (str (+ t.index 1))
              :key (str "track-mute-" t.index)
              :width 1.55 :height 1.2 :padding 0 :font-size 10
              :active #'t.muted
              :background-color (mute-button-bg false)
              :active-background-color (mute-button-bg true)
              :color :blue
              :active-color :gray
              :on-click |x y r| (do (set! eseq.seq-core-state/selected-bus -1) (toggle! t.muted)))
            (button "S"
              :key (str "track-solo-" t.index)
              :width 1.55 :height 1.2 :padding 0 :font-size 10
              :active #'t.soloed
              :background-color (solo-button-bg false)
              :active-background-color (solo-button-bg true)
              :color :gray
              :active-color :white
              :on-click |x y r| (do (set! eseq.seq-core-state/selected-bus -1) (toggle! t.soloed)))
            (box :width 8.6 :height 1
              :key (str "track-select-" t.index)
              :bg (if (current? t) :blue :dark-gray)
              :on-click |x y r| (do (set! eseq.seq-core-state/selected-bus -1) (set! selection.track t))
              ;; Dark while silent (muted or soloed away): lit while heard.
              (label (substring t.name 0 12) :font-size 11 :width 8.6
                :active #'t.audible
                :color :dark-gray
                :active-color (if (current? t) :white :gray)
                :bg :transparent))
            (box :width 5.2
              (v-stack :gap 0.18
                (hslider :min 0 :max 1 :width 5
                  :key (str "track-volume-" t.index)
                  :value #'t.volume
                  :material (eseq.materials/slider-material)
                  :on-change (lambda (v) (do (set! eseq.seq-core-state/selected-bus -1) (set! t.volume v))))
                (subtree :key (str "mixer-track-meter-" t.index)
                  (mixer-track-meter :level #'t.peak))))
            (if (and (current? t) (> (len (tracks)) 1))
              (box :width 1.6 :height 1.2 :align :center
                :bg :transparent
                :key (str "track-delete-" t.index)
                :on-click |x y r| (host-command "delete-track" (dict :track t.index))
                :background "delete-track-icon"
                :active 0)
              (label "" :width 1.6 :bg :transparent))))))
    (each (display-buses) |b|
      (subtree :key (str "mixer-bus-row-" b.index)
        (box :background "track-container"
          :padding 0.5
          :even (mod b.index 2)
          :selected (if (= eseq.seq-core-state/selected-bus b.index) 1 0)
          (h-stack :gap 0.5 :align :center
            (label "" :width 2 :height 1.5 :bg :transparent)
            (button (bus-row-label b)
              :key (str "bus-mute-" b.index)
              :width 1.55 :height 1.2 :padding 0 :font-size 10
              :active #'b.muted
              :background-color (mute-button-bg false)
              :active-background-color (mute-button-bg true)
              :color :blue
              :active-color :gray
              :on-click |x y r| (toggle! b.muted))
            (button "S"
              :key (str "bus-solo-" b.index)
              :width 1.55 :height 1.2 :padding 0 :font-size 10
              :active #'b.soloed
              :background-color (solo-button-bg false)
              :active-background-color (solo-button-bg true)
              :color :gray
              :active-color :white
              :on-click |x y r| (toggle! b.soloed))
            (box :width 8.6 :height 1
              :key (str "bus-select-" b.index)
              :bg (if (= eseq.seq-core-state/selected-bus b.index) :blue :dark-gray)
              :on-click |x y r| (do (seq-clear-selection) (set! eseq.seq-core-state/selected-bus b.index))
              (label (substring b.name 0 12) :font-size 11 :width 8.6
                :color (if (= eseq.seq-core-state/selected-bus b.index) :white :gray)
                :bg :transparent))
            (box :width 5.2
              (hslider :min 0 :max 1 :width 5
                :key (str "bus-volume-" b.index)
                :value #'b.volume
                :material (eseq.materials/slider-material)
                :on-change (lambda (v) (set! b.volume v))))
            (label "" :width 1.6 :bg :transparent)))))))
