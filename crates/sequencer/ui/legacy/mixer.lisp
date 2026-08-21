;; ui/legacy/mixer.lisp - retained predecessor to ui/mixer.lisp; NOT loaded by
;; ui/main.lisp, and (unlike step-grid.lisp) not even in the metal_seq parse
;; gate's file list. Renders to the *mixer* buffer when evaluated directly.
;;
;; Converted in S3b wave 10. Notes specific to this file:
;;
;;   * Dead reference code: a whole-tree sweep of crates/sequencer/src,
;;     crates/eseqlisp/src and crates/sequencer/ui found ZERO callers of any
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
;;     implicit-module compiler (hazard g/h), so their `aqua-color` reference
;;     stays FLAT through ui/materials.lisp's compat alias — likewise the
;;     `(aqua-slider-material)` calls in the `:material` props. Do not import
;;     eseq.materials and requalify them; that breaks them.
;;   * `selected-bus` stays bare (including the `set!`s): it is
;;     eseq.seq-core-state's `defstate`, and `defstate` resolves on the flat
;;     key through `state_bindings`, so hazards (j)/(m) do not fire.
;;   * No `import` lines at all — everything this file reads from outside is
;;     either a Rust native (`reactive-get`, the `seq-*` family) or a
;;     `defstate`. That also makes the file trivially safe under hazards
;;     (n)/(n2), neither of which applies since no Rust harness reads it.
;;   * Widget `:key` props auto-qualify, so the hand-rolled `mixer-` prefix is
;;     stripped from them; `(subtree :key …)` strings are left byte-identical
;;     (hazard a), as is the `"*mixer*"` buffer name.

(module eseq.legacy.mixer)

(def track-peak (i)
  (reactive-get "SEQ" (str "track-peak-" i)))

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
    )

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

(def bus-row-label (i)
  (if (= i 0) "M" (if (= i 1) "A" (if (= i 2) "B" (str i)))))

(def has-mix-bus? ()
  (and (> (len SEQ.bus-names) 0) (= (nth SEQ.bus-names 0) "Mix")))

(def display-bus-index (display-i)
  (if (or (not (has-mix-bus?)) (<= (len SEQ.bus-names) 1))
    display-i
    (if (= display-i (- (len SEQ.bus-names) 1))
      0
      (+ display-i 1))))

(effect-buffer "*mixer*"
  (v-stack :padding 0.5 :gap 0.25
    (each (range 0 (+ SEQ.num-tracks (len SEQ.bus-names))) |row|
      (if (< row SEQ.num-tracks)
        (let ((i row)
              (name (nth SEQ.track-names row)))
          (subtree :key (str "mixer-track-row-" i)
            (box :background "track-container"
              :padding 0.5
              :even (mod i 2)
              :selected (if (and (< eseq.seq-core-state/selected-bus 0) (= SEQ.current-track i)) 1 0)

              (h-stack :gap 0.5 :align :center
                (box :width 2 :height 1.5
                  :background "rec-arm-dot"
                  :key (str "track-arm-" i)
                  :active (if (nth SEQ.record-armed i) 1 0)
                  :on-click |x y r| (do (set! eseq.seq-core-state/selected-bus -1) (seq-toggle-record-arm i)))
                (button (str (+ i 1))
                  :key (str "track-mute-" i)
                  :width 1.55 :height 1.2 :padding 0 :font-size 10
                  :background-color (mute-button-bg (nth SEQ.track-mutes i))
                  :color (if (nth SEQ.track-mutes i) :gray :blue)
                  :on-click |x y r| (do (set! eseq.seq-core-state/selected-bus -1) (seq-toggle-track-mute i)))
                (button "S"
                  :key (str "track-solo-" i)
                  :width 1.55 :height 1.2 :padding 0 :font-size 10
                  :background-color (solo-button-bg (nth SEQ.track-solos i))
                  :color (if (nth SEQ.track-solos i) :white :gray)
                  :on-click |x y r| (do (set! eseq.seq-core-state/selected-bus -1) (seq-toggle-track-solo i)))
                (box :width 8.6 :height 1
                  :key (str "track-select-" i)
                  :bg (if (and (< eseq.seq-core-state/selected-bus 0) (= SEQ.current-track i)) :blue :dark-gray)
                  :on-click |x y r| (do (set! eseq.seq-core-state/selected-bus -1) (seq-set-track i))
                  (label (substring name 0 12) :font-size 11 :width 8.6
                    :color (if (or (nth SEQ.track-mutes i) (nth SEQ.track-muted-by-solo i))
                             :dark-gray
                             (if (and (< eseq.seq-core-state/selected-bus 0) (= SEQ.current-track i)) :white :gray))
                    :bg :transparent))
                (box :width 5.2
                  (v-stack :gap 0.18
                    (hslider :min 0 :max 1 :width 5
                      :key (str "track-volume-" i)
                      :value (nth SEQ.track-volumes i)
                      :material (eseq.materials/slider-material)
                      :on-change (lambda (v) (do (set! eseq.seq-core-state/selected-bus -1) (seq-set-track-volume i v))))
                    (subtree :key (str "mixer-track-meter-" i)
                      (mixer-track-meter :level (track-peak i)))))
                (if (and (< eseq.seq-core-state/selected-bus 0) (= SEQ.current-track i) (> SEQ.num-tracks 1))
                  (box :width 1.6 :height 1.2 :align :center
                    :bg :transparent
                    :key (str "track-delete-" i)
                    :on-click |x y r| (host-command "delete-track" (dict :track i))
                    :background "delete-track-icon"
                    :active 0)
                  (label "" :width 1.6 :bg :transparent))))))
        (let ((display-i (- row SEQ.num-tracks))
              (i (display-bus-index (- row SEQ.num-tracks)))
              (name (nth SEQ.bus-names i)))
          (subtree :key (str "mixer-bus-row-" i)
            (box :background "track-container"
              :padding 0.5
              :even (mod row 2)
              :selected (if (= eseq.seq-core-state/selected-bus i) 1 0)
              (h-stack :gap 0.5 :align :center
                (label "" :width 2 :height 1.5 :bg :transparent)
                (button (bus-row-label i)
                  :key (str "bus-mute-" i)
                  :width 1.55 :height 1.2 :padding 0 :font-size 10
                  :background-color (mute-button-bg (nth SEQ.bus-mutes i))
                  :color (if (nth SEQ.bus-mutes i) :gray :blue)
                  :on-click |x y r| (seq-toggle-bus-mute i))
                (button "S"
                  :key (str "bus-solo-" i)
                  :width 1.55 :height 1.2 :padding 0 :font-size 10
                  :background-color (solo-button-bg (nth SEQ.bus-solos i))
                  :color (if (nth SEQ.bus-solos i) :white :gray)
                  :on-click |x y r| (seq-toggle-bus-solo i))
                (box :width 8.6 :height 1
                  :key (str "bus-select-" i)
                  :bg (if (= eseq.seq-core-state/selected-bus i) :blue :dark-gray)
                  :on-click |x y r| (do (seq-clear-selection) (set! eseq.seq-core-state/selected-bus i))
                  (label (substring name 0 12) :font-size 11 :width 8.6
                    :color (if (= eseq.seq-core-state/selected-bus i) :white :gray)
                    :bg :transparent))
                (box :width 5.2
                  (hslider :min 0 :max 1 :width 5
                    :key (str "bus-volume-" i)
                    :value (nth SEQ.bus-volumes i)
                    :material (eseq.materials/slider-material)
                    :on-change (lambda (v) (seq-set-bus-volume i v))))
                (label "" :width 1.6 :bg :transparent))))))))))
