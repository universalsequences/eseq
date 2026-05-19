;; metal-seq-fx.lisp — Effect chain UI for Metal Sequencer
;; Renders to *fx* buffer. Loaded by metal-seq-grid.lisp.

(defstate instrument-panel-tab 0)
(defstate instrument-source-tab 0)
(defstate instrument-mods-open false)
(defstate instrument-selected-mod-slot 1)
(defstate selected-fx-slot -1)
(defstate selected-midi-fx-slot -1)
(defstate selected-bus-fx-slot -1)
;; These are temporary render-context globals used by generated custom synth UI.
;; They must NOT be defstate: custom UI functions set them while rendering, and
;; writing reactive state during measurement/layout can perturb the layout.
(def synth-ui-current-inst false)
(def synth-ui-current-name "")
(def midi-fx-ui-current-fx false)
(def midi-fx-ui-current-name "")
(def audio-fx-ui-current-fx false)
(def audio-fx-ui-current-name "")
(def custom-ui-current-kind "instrument")

;; Matches a standard built-in FX panel with four parameter rows.
(def fx-fixed-panel-height 9.95)
(def fx-panel-body-padding 0.35)

(def fx-panel-body (debug-name children)
  (box
    (v-stack :gap 0
      (box :width 1 :height 0.16)
      children)
    :debug-name debug-name
    :on-click (lambda (info) (fx-clear-selected-effect))
    :padding fx-panel-body-padding
    :v-align :start
    :h-align :start))

(def fx-panel-header-leading-spacer ()
  (box :width 0.4 :height 0))

(def fx-effect-chain-kind (fx)
  (if (get fx :midi-fx)
    "midi"
    (if (get fx :bus-fx) "bus" "audio")))

(def fx-effect-drag-kind (fx)
  (if (get fx :midi-fx)
    "midi-effect-instance"
    (if (get fx :bus-fx) "bus-effect-instance" "audio-effect-instance")))

(def fx-effect-drag-payload (fx title)
  (dict :kind (fx-effect-drag-kind fx)
        :chain (fx-effect-chain-kind fx)
        :track SEQ.current-track
        :bus (if (get fx :bus-fx) (get fx :bus-idx) -1)
        :slot (get fx :slot-idx)
        :name title
        :builtin (get fx :builtin)))

(def fx-effect-drop-meta (fx)
  (dict :kind "fx-slot"
        :chain (fx-effect-chain-kind fx)
        :track SEQ.current-track
        :bus (if (get fx :bus-fx) (get fx :bus-idx) -1)
        :slot (get fx :slot-idx)))

(def fx-effect-drop-types (fx)
  (if (get fx :midi-fx)
    (list "midi-effect" "effect-instance")
    (if (get fx :bus-fx)
      (list "audio-effect")
      (list "audio-effect" "effect-instance"))))

(def fx-panel-header (title params fx)
  (box :width :fill :height 0.7 :padding 0 :v-align :center :h-align :start
    :debug-name (if (get fx :midi-fx) "midi-fx-panel-header" "audio-fx-panel-header")
    :drag-type "effect-instance"
    :drag-payload (fx-effect-drag-payload fx title)
    :on-click (lambda (info)
      (if (get fx :midi-fx)
        (fx-select-midi-effect (get fx :slot-idx))
        (if (get fx :bus-fx)
          (fx-select-bus-effect (get fx :slot-idx))
          (fx-select-effect (get fx :slot-idx)))))
    (h-stack :gap 0.5 :align :center
      (fx-panel-header-leading-spacer)
      (fx-enabled-toggle (enabled-param params) fx
        (if (get fx :midi-fx)
          (str "midi-fx-enabled-" (get fx :slot-idx))
          (if (get fx :bus-fx)
            (str "bus-fx-enabled-" (get fx :bus-idx) "-" (get fx :slot-idx))
            (str "audio-fx-enabled-" (get fx :slot-idx)))))
      (label title :font-size 11 :color :white :bg :transparent)
      (if (and (not (get fx :midi-fx)) (not (get fx :builtin)))
        (box :bg :dark-gray :width 4 :height 1.0 :align :center
          :on-click (lambda (info)
            (do
              (fx-clear-selected-effect)
              (host-command "enter-edit-effect"
                (if (get fx :bus-fx)
                  (dict :name title :slot (get fx :slot-idx) :bus (get fx :bus-idx))
                  (dict :name title :slot (get fx :slot-idx))))))
          (label "edit" :font-size 8 :color :dim :bg :transparent))
        (box)))))

(def fx-clear-selected-effect ()
  (do
    (set! selected-fx-slot -1)
    (set! selected-midi-fx-slot -1)
    (set! selected-bus-fx-slot -1)))

(def fx-drop-library-effect (payload target)
  (let ((kind (get payload :kind))
        (name (get payload :name))
        (chain (get target :chain))
        (track (get target :track))
        (slot (get target :slot)))
    (if (= chain "append")
      (if (= kind "builtin-audio-effect")
        (host-command "add-builtin-effect-to-track" (dict :track track :name name))
        (if (= kind "custom-audio-effect")
          (host-command "add-effect-to-track" (dict :track track :name name))
          (if (= kind "midi-effect")
            (host-command "add-midi-fx-to-track" (dict :track track :name name))
            (status "Drop an audio or MIDI effect"))))
      (if (and (= chain "audio") (= kind "builtin-audio-effect"))
        (host-command "insert-builtin-effect-before-slot" (dict :track track :slot slot :name name))
        (if (and (= chain "audio") (= kind "custom-audio-effect"))
          (host-command "insert-effect-before-slot" (dict :track track :slot slot :name name))
          (if (and (= chain "midi") (= kind "midi-effect"))
            (host-command "insert-midi-fx-before-slot" (dict :track track :slot slot :name name))
            (status "That effect type does not belong in this chain")))))))

(def fx-drop-existing-effect (payload target)
  (let ((kind (get payload :kind))
        (source-chain (get payload :chain))
        (source-track (get payload :track))
        (source-slot (get payload :slot))
        (target-chain (get target :chain))
        (target-track (get target :track))
        (target-slot (get target :slot)))
    (if (and (= kind "audio-effect-instance") (= source-chain "audio")
             (or (= target-chain "audio") (= target-chain "append")))
      (host-command "move-effect-slot"
        (dict :source-track source-track :source-slot source-slot
              :target-track target-track :target-slot target-slot
              :position target-chain))
      (if (and (= kind "midi-effect-instance") (= source-chain "midi")
               (or (= target-chain "midi") (= target-chain "append")))
        (host-command "move-midi-fx-slot"
          (dict :source-track source-track :source-slot source-slot
                :target-track target-track :target-slot target-slot
                :position target-chain))
        (status "Move effects within the same audio or MIDI chain")))))

(def fx-drop-on-effect (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (let ((kind (get payload :kind)))
      (if (or (= kind "audio-effect-instance") (= kind "midi-effect-instance") (= kind "bus-effect-instance"))
        (fx-drop-existing-effect payload target)
        (fx-drop-library-effect payload target)))))

(def fx-track-bus-send-control (send)
  (v-stack :align :center :gap 0.25
    (h-stack :gap 0.25 :align :baseline
      (label (substring (get send :name) 0 8) :font-size 9 :color :dim :bg :transparent)
      (number-picker :value (get send :amount) :min 0 :max 1 :decimals 2
        :noui true :font-size 9 :text-color :dim
        :on-change (lambda (v)
          (do
            (cool-off-follow)
            (host-command "set-track-bus-send"
              (dict :bus (get send :bus-idx) :amount v))))
        :width 4 :height 1))
    (box :width 8 :height 2
      (hslider :min 0 :max 1
        :value (get send :amount)
        :material (aqua-slider-material)
        :on-change (lambda (v)
          (do
            (cool-off-follow)
            (host-command "set-track-bus-send"
              (dict :bus (get send :bus-idx) :amount v))))))))

(def fx-plock-set-value (p v)
  (do
    (cool-off-follow)
    (host-command "set-track-plock-entry"
      (dict :target (get p :target)
            :step-idx (get p :step-idx)
            :slot-idx (get p :slot-idx)
            :param-idx (get p :param-idx)
            :value v))))

(def fx-plock-set-option (p label)
  (do
    (cool-off-follow)
    (host-command "set-track-plock-entry-option"
      (dict :target (get p :target)
            :step-idx (get p :step-idx)
            :slot-idx (get p :slot-idx)
            :param-idx (get p :param-idx)
            :label label))))

(def fx-plock-clear (p)
  (do
    (cool-off-follow)
    (host-command "clear-track-plock-entry"
      (dict :target (get p :target)
            :step-idx (get p :step-idx)
            :slot-idx (get p :slot-idx)
            :param-idx (get p :param-idx)))))

(def fx-plock-row (p idx)
  (subtree :key (str "track-plock-" idx "-" (get p :target) "-" (get p :step-idx) "-"
                     (get p :slot-idx) "-" (get p :param-idx))
    (box :height 1.28
      (h-stack :gap 0.35 :align :center
        (label (str "S" (get p :step)) :font-size 9 :width 2.2 :color :yellow :bg :transparent)
        (label (substring (get p :group) 0 8) :font-size 9 :width 5.6 :color :dim :bg :transparent)
        (label (substring (get p :name) 0 9) :font-size 10 :width 5.9 :color :white :bg :transparent)
        (if (get p :options)
          (dropdown :value (get p :text-value)
            :options (get p :options)
            :on-change (lambda (v) (fx-plock-set-option p v))
            :width 5.2 :height 1.1 :font-size 9)
          (number-picker :value (get p :value)
            :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 2
            :noui true :font-size 10 :text-color :dim
            :on-change (lambda (v) (fx-plock-set-value p v))
            :width 4.5 :height 1.05))
        (button "x"
          :width 1.35 :height 1.05 :padding 0 :font-size 9
          :background-color :dark-gray :color :dim
          :on-click |x y r| (fx-plock-clear p))))))

(def fx-track-plocks-panel ()
  (box :debug-name "track-plocks-panel" :padding 0.75
    (v-stack :gap 0.35
      (h-stack :gap 0.35 :align :baseline
        (label "p-locks" :font-size 10 :color :white :bg :transparent)
        (label (str (len SEQ.track-plocks))
          :font-size 8 :color :dim :bg :transparent))
      (if (> (len SEQ.track-plocks) 0)
        (v-stack :gap 0.2
          (each SEQ.track-plocks |p idx|
            (fx-plock-row p idx)))
        (label "no p-locks for selected steps" :font-size 9 :color :dim :bg :transparent)))))

(def fx-track-accumulator-panel ()
  (box :debug-name "track-accumulator-panel" :padding 0.75
    (h-stack :gap 0.55 :align :center
      (v-stack :align :center :gap 0.30
        (label "acc fn" :font-size 8 :color :dim :bg :transparent)
        (dropdown :value SEQ.tp-accumulator
          :options SEQ.accumulator-options
          :on-change (lambda (v) (do (cool-off-follow) (seq-set-accumulator v)))
          :width 7.0 :height 1.25 :font-size 9))
      (v-stack :align :center :gap 0.30
        (label "acc mode" :font-size 8 :color :dim :bg :transparent)
        (dropdown :value SEQ.tp-accum-mode
          :options SEQ.accum-mode-options
          :on-change (lambda (v) (do (cool-off-follow) (seq-set-accum-mode v)))
          :width 6.0 :height 1.25 :font-size 9))
      (v-stack :align :center :gap 0.22
        (h-stack :gap 0.2 :align :baseline
          (label "acc lim" :font-size 8 :color :dim :bg :transparent)
          (number-picker :value SEQ.tp-accum-limit :min 0 :max 127 :decimals 0
            :noui true :font-size 8 :text-color :dim
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-accum-limit v)))
            :width 3.2 :height 0.85))
        (box :width 5.8 :height 1.2
          (hslider :min 0 :max 127
            :value SEQ.tp-accum-limit
            :material (aqua-slider-material)
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-accum-limit v)))))))))

(def fx-track-parameters-panel ()
  (box :debug-name "track-parameters-strip" :padding 0.9
    (v-stack :gap 0.75
      (h-stack :gap 0.55 :align :center
        (v-stack :align :center :gap 0.22
          (h-stack :gap 0.2 :align :baseline
            (label "steps" :font-size 8 :color :dim :bg :transparent)
            (number-picker :value SEQ.tp-num-steps :min 1 :max 256 :decimals 0
              :noui true :font-size 8 :text-color :dim
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :num-steps v)))
              :width 3.2 :height 0.85))
          (box :width 6.0 :height 1.2
            (hslider :min 1 :max 256
              :value SEQ.tp-num-steps
              :material (aqua-slider-material)
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :num-steps v))))))
        (v-stack :align :center :gap 0.24
          (label "poly" :font-size 8 :color :dim :bg :transparent)
          (box :width 3.2 :height 1.3
            :bg (if SEQ.tp-poly :blue :dark-gray)
            :on-click |x y r| (do (cool-off-follow) (seq-set-track-param :poly (if SEQ.tp-poly 0 1)))
            (label (if SEQ.tp-poly "ON" "OFF") :font-size 9 :color :white :bg :transparent)))
        (v-stack :align :center :gap 0.22
          (h-stack :gap 0.2 :align :baseline
            (label "voices" :font-size 8 :color :dim :bg :transparent)
            (number-picker :value SEQ.tp-max-polyphony :min 1 :max 12 :decimals 0
              :noui true :font-size 8 :text-color :dim
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :voices v)))
              :width 2.4 :height 0.85))
          (box :width 4.8 :height 1.2
            (hslider :min 1 :max 12
              :value SEQ.tp-max-polyphony
              :material (aqua-slider-material)
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :voices v))))))
        (v-stack :align :center :gap 0.30
          (label "fts" :font-size 8 :color :dim :bg :transparent)
          (dropdown :value SEQ.tp-fts
            :options SEQ.fts-options
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-fts v)))
            :width 7.0 :height 1.25 :font-size 9)))
      (h-stack :gap 0.55 :align :center
        (v-stack :align :center :gap 0.30
          (label "swg res" :font-size 8 :color :dim :bg :transparent)
          (dropdown :value SEQ.tp-swing-resolution
            :options '("1/16" "1/8" "1/4" "1/2")
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-swing-resolution v)))
            :width 5.0 :height 1.25 :font-size 9))
        (v-stack :align :center :gap 0.22
          (h-stack :gap 0.2 :align :baseline
            (label "swg" :font-size 8 :color :dim :bg :transparent)
            (number-picker :value SEQ.tp-swing :min 50 :max 75 :decimals 1
              :noui true :font-size 8 :text-color :dim
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :swing v)))
              :width 3.2 :height 0.85))
          (box :width 5.8 :height 1.2
            (hslider :min 50 :max 75
              :value SEQ.tp-swing
              :material (aqua-slider-material)
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :swing v))))))))))

(def fx-select-effect (slot)
  (do
    (set! selected-fx-slot slot)
    (set! selected-midi-fx-slot -1)
    (set! selected-bus-fx-slot -1)))

(def fx-select-midi-effect (slot)
  (do
    (set! selected-midi-fx-slot slot)
    (set! selected-fx-slot -1)
    (set! selected-bus-fx-slot -1)))

(def fx-select-bus-effect (slot)
  (do
    (set! selected-bus-fx-slot slot)
    (set! selected-fx-slot -1)
    (set! selected-midi-fx-slot -1)))

(def fx-has-selected-bus? ()
  (and (>= selected-bus 0)
       (< selected-bus (len SEQ.bus-names))
       (< selected-bus (len SEQ.bus-effects))))

(def fx-delete-selected-effect ()
  (if (and (fx-has-selected-bus?) (>= selected-bus-fx-slot 0))
    (do
      (host-command "delete-bus-effect"
        (dict :bus selected-bus :slot selected-bus-fx-slot))
      (fx-clear-selected-effect))
  (if (>= selected-midi-fx-slot 0)
    (do
      (host-command "delete-midi-fx" (dict :slot selected-midi-fx-slot))
      (fx-clear-selected-effect))
    (if (>= selected-fx-slot 0)
    (do
      (host-command "delete-effect" (dict :slot selected-fx-slot))
      (fx-clear-selected-effect))
    (fx-clear-selected-effect)))))

(defwidget fx-panel-bg
  :width 1 :height 1
  :state (selected header-r header-g header-b selected-header-r selected-header-g selected-header-b)
  :shader
  (let ((panel-radius (min (* 3 (fwidth y)) (* 0.5 (min width height))))
      (panel (sdf/rounded-rect (* 1 width) (* 1 height) (* 2 panel-radius)))
      ;; Use derivatives to convert a real pixel height into the shader's
      ;; normalized/SDF y-space. This keeps the header bar visually constant
      ;; as panels get taller/shorter.
      (header-h (* 35 (fwidth y)))
      (header-bottom (+ (- height) header-h))
      (header-shape (max panel (- y header-bottom))))
    (sdf/layer
      (sdf/fill
        panel
        (material
          :color
          (let ((header-aa (max (fwidth header-shape) (fwidth y)))
                (border-w (max (* 1.5 (fwidth d)) (fwidth y)))
                (body input-color)
                (header (rgba header-r header-g header-b 1.0))
                (base
                  (mix header body (smoothstep 0 header-aa header-shape))))
            base)))
      (if selected
        (sdf/fill header-shape
          (material :color (rgba selected-header-r selected-header-g selected-header-b 1.0)))
        (rgba 0 0 0 0)))))

(defwidget compile-progress
  :width 12 :height 0.3
  :state (active)
  :shader
  (if (= active 0)
    (rgba 0 0 0 0)
    (let ((bar-w 0.3)
          (pos (fract (* 0.5 itime)))
          (bar-x (- (* pos (+ 1 bar-w)) (/ bar-w 2)))
          (d-bar (- (abs (- x bar-x)) (/ bar-w 2)))
          (bg (sdf/rounded-rect width height 0.06))
          (mask (max bg (- d-bar))))
      (sdf/layer
        (sdf/fill bg
          (material :color (rgba 0.15 0.15 0.17 1)))
          (sdf/fill mask
          (material :color
            (mix
              (rgba 0.3 0.5 1.0 1)
              (rgba 0.2 0.35 0.8 1)
              (smoothstep -0.02 0.02 d-bar))))))))

(defwidget agent-instrument-stub-bg
  :width 70 :height 1
  :paint-margin 0.2
  :shader
  (let ((drift (* itime 0.11))
        (pulse (+ 0.5 (* 0.5 (sin (* itime 0.72)))))
        (sx (+ (* x 0.17) drift))
        (sy (+ (* y aspect 0.92) (* (sin (* itime 0.19)) 0.36)))
        (ix (floor sx))
        (iy (floor sy))
        (fx (fract sx))
        (fy (fract sy))
        (ux (smoothstep 0.0 1.0 fx))
        (uy (smoothstep 0.0 1.0 fy))
        (h00 (fract (* (sin (+ (* ix 127.1) (* iy 311.7))) 43758.5453)))
        (h10 (fract (* (sin (+ (* (+ ix 1.0) 127.1) (* iy 311.7))) 43758.5453)))
        (h01 (fract (* (sin (+ (* ix 127.1) (* (+ iy 1.0) 311.7))) 43758.5453)))
        (h11 (fract (* (sin (+ (* (+ ix 1.0) 127.1) (* (+ iy 1.0) 311.7))) 43758.5453)))
        (n0 (mix h00 h10 ux))
        (n1 (mix h01 h11 ux))
        (cloud-a (mix n0 n1 uy))
        (sx2 (+ (* x 0.39) (* (sin (* itime 0.13)) 0.8)))
        (sy2 (- (* y aspect 1.8) (* itime 0.17)))
        (ix2 (floor sx2))
        (iy2 (floor sy2))
        (fx2 (fract sx2))
        (fy2 (fract sy2))
        (ux2 (smoothstep 0.0 1.0 fx2))
        (uy2 (smoothstep 0.0 1.0 fy2))
        (k00 (fract (* (sin (+ (* ix2 269.5) (* iy2 183.3))) 24634.6345)))
        (k10 (fract (* (sin (+ (* (+ ix2 1.0) 269.5) (* iy2 183.3))) 24634.6345)))
        (k01 (fract (* (sin (+ (* ix2 269.5) (* (+ iy2 1.0) 183.3))) 24634.6345)))
        (k11 (fract (* (sin (+ (* (+ ix2 1.0) 269.5) (* (+ iy2 1.0) 183.3))) 24634.6345)))
        (m0 (mix k00 k10 ux2))
        (m1 (mix k01 k11 ux2))
        (cloud-b (mix m0 m1 uy2))
        (cloud (smoothstep 0.18 0.92 (+ (* cloud-a 0.68) (* cloud-b 0.32))))
        (body (rgba 0.055 0.060 0.072 1.0))
        (blue (rgba 0.05 0.30 0.48 1.0))
        (violet (rgba 0.48 0.20 0.56 1.0))
        (cyan (rgba 0.15 0.78 0.92 1.0))
        (magenta (rgba 0.96 0.34 0.74 1.0)))
    (sdf/layer
      (sdf/fill
        (sdf/rect width height)
        (material :color
          (mix
            (mix body (mix blue violet pulse) 0.30)
            (mix cyan magenta pulse)
            (* cloud 0.42)))))))

(def fx-set-instrument-value (p v)
  (do
    (fx-clear-selected-effect)
    (if (= (get p :control) "base-note")
      (host-command "set-instrument-base-note" (dict :value v))
      (host-command
        (if (seq-has-selection?) "set-instrument-plock" "set-instrument-param")
        (dict :param-idx (get p :idx) :value v)))))

(def fx-set-instrument-option (p label)
  (do
    (fx-clear-selected-effect)
    (host-command
      (if (seq-has-selection?) "set-instrument-plock-option" "set-instrument-param-option")
      (dict :param-idx (get p :idx) :label label))))

(def custom-ui-option-index (options label)
  (nth (filter |idx| (= (nth options idx) label) (range (len options))) 0))

(def fx-set-effect-value (fx p v)
  (do
    (fx-clear-selected-effect)
    (if (get fx :bus-fx)
      (host-command (if (seq-has-selection?) "set-bus-effect-plock" "set-bus-effect-param")
        (dict :bus (get fx :bus-idx) :slot-idx (get fx :slot-idx)
              :param-idx (get p :idx) :value v))
    (if (get fx :midi-fx)
      (host-command
        (if (seq-has-selection?) "set-midi-fx-plock" "set-midi-fx-param")
        (dict :slot-idx (get fx :slot-idx) :param-idx (get p :idx) :value v))
      (if (seq-has-selection?)
        (seq-set-effect-plock (get fx :slot-idx) (get p :idx) v)
        (host-command "set-effect-param"
          (dict :slot-idx (get fx :slot-idx) :param-idx (get p :idx) :value v)))))))

(def fx-toggle-instrument-value (p)
  (do
    (fx-clear-selected-effect)
    (host-command "toggle-instrument-param"
      (dict :param-idx (get p :idx)))))

(def fx-toggle-effect-value (fx p)
  (do
    (fx-clear-selected-effect)
    (host-command "toggle-effect-param"
      (dict :bus (get fx :bus-idx)
            :bus-fx (get fx :bus-fx)
            :midi-fx (get fx :midi-fx)
            :slot-idx (get fx :slot-idx)
            :param-idx (get p :idx)))))

(def fx-param-value (p)
  (if (and instrument-mods-open (get p :modulatable))
    (let ((target (instrument-param-control-mod-target p)))
      (if target (instrument-mod-target-depth target) 0))
    (if (get p :value-field)
      (bind-seq (get p :value-field))
      (get p :value))))

(def fx-param-numeric-value (p)
  (reactive-value (fx-param-value p)))

(def fx-param-on? (p)
  (> (fx-param-numeric-value p) 0.5))

(def instrument-mod-selected-slot ()
  (if (> instrument-selected-mod-slot 0) instrument-selected-mod-slot 1))

(def instrument-param-mod-targets (p)
  (if (get p :mod-targets) (get p :mod-targets) '()))

(def instrument-mod-target-source-slot (target)
  (let ((slot (if (get target :source-value-field)
                (reactive-value (bind-seq (get target :source-value-field)))
                (get target :source-slot))))
    (if slot slot (get target :source-slot))))

(def instrument-mod-target-depth (target)
  (if (get target :depth-value-field)
    (bind-seq (get target :depth-value-field))
    (get target :depth)))

(def instrument-param-base-value (p)
  (if (get p :value-field)
    (bind-seq (get p :value-field))
    (get p :value)))

(def instrument-param-active-mod-targets (p)
  (if (and instrument-mods-open (get p :modulatable))
    (filter |target| (> (instrument-mod-target-source-slot target) 0)
      (instrument-param-mod-targets p))
    '()))

(def instrument-param-knob-mod-target (p idx)
  (if (and instrument-mods-open (get p :modulatable))
    (nth (instrument-param-mod-targets p) idx)
    false))

(def instrument-param-knob-mod-slot-prop (p idx)
  (let ((target (instrument-param-knob-mod-target p idx)))
    (if target (instrument-mod-target-source-slot target) false)))

(def instrument-param-knob-mod-depth-prop (p idx)
  (let ((target (instrument-param-knob-mod-target p idx)))
    (if target (instrument-mod-target-depth target) false)))

(def instrument-param-base-value-prop (p)
  (if (and instrument-mods-open (get p :modulatable))
    (instrument-param-base-value p)
    false))

(def instrument-param-base-min-prop (p)
  (if (and instrument-mods-open (get p :modulatable))
    (get p :min)
    false))

(def instrument-param-base-max-prop (p)
  (if (and instrument-mods-open (get p :modulatable))
    (get p :max)
    false))

(def instrument-selected-mod-slot-prop (p)
  (if (and instrument-mods-open (get p :modulatable))
    (instrument-mod-selected-slot)
    false))

(def instrument-param-control-key-mode (p)
  (if (and instrument-mods-open (get p :modulatable))
    "-mod-depth"
    "-base"))

(def instrument-param-selected-mod-target (p)
  (nth
    (filter |target| (= (instrument-mod-target-source-slot target) (instrument-mod-selected-slot))
      (instrument-param-mod-targets p))
    0))

(def instrument-param-empty-mod-target (p)
  (nth
    (filter |target| (and (get target :source-idx)
                          (= (instrument-mod-target-source-slot target) 0))
      (instrument-param-mod-targets p))
    0))

(def instrument-param-control-mod-target (p)
  (let ((selected-target (instrument-param-selected-mod-target p)))
    (if selected-target
      selected-target
      (let ((empty-target (instrument-param-empty-mod-target p)))
        (if empty-target
          empty-target
          (nth (instrument-param-mod-targets p) 0))))))

(def instrument-param-connected-to-selected-mod? (p)
  (if (instrument-param-selected-mod-target p) true false))

(def instrument-param-connected-to-other-mod? (p)
  (> (len
      (filter |target|
        (and (> (instrument-mod-target-source-slot target) 0)
             (not (= (instrument-mod-target-source-slot target) (instrument-mod-selected-slot))))
        (instrument-param-mod-targets p)))
     0))

(def instrument-param-control-min (p)
  (if (and instrument-mods-open (get p :modulatable))
    (let ((target (instrument-param-control-mod-target p)))
      (if target (get target :depth-min) -1))
    (get p :min)))

(def instrument-param-control-max (p)
  (if (and instrument-mods-open (get p :modulatable))
    (let ((target (instrument-param-control-mod-target p)))
      (if target (get target :depth-max) 1))
    (get p :max)))

(def instrument-set-param-control-value (p v)
  (if (and instrument-mods-open (get p :modulatable))
    (let ((target (instrument-param-control-mod-target p)))
      (if target
        (let ((source-slot (instrument-mod-target-source-slot target)))
          (if (= source-slot (instrument-mod-selected-slot))
            (fx-set-instrument-value
              (dict :idx (get target :depth-idx) :control "param")
              v)
            (if (= source-slot 0)
              (do
                (fx-set-instrument-value
                  (dict :idx (get target :source-idx) :control "param")
                  (instrument-mod-selected-slot))
                (fx-set-instrument-value
                  (dict :idx (get target :depth-idx) :control "param")
                  v)))))))
    (fx-set-instrument-value p v)))

(def instrument-toggle-param-modulation (p)
  (if (get p :modulatable)
    (let ((target (instrument-param-selected-mod-target p)))
      (if target
        (if (get target :source-idx)
          (fx-set-instrument-value
            (dict :idx (get target :source-idx) :control "param")
            0)
          (fx-set-instrument-value
            (dict :idx (get target :depth-idx) :control "param")
            0))
        (let ((target (instrument-param-empty-mod-target p)))
          (if target
            (do
              (fx-set-instrument-value
                (dict :idx (get target :source-idx) :control "param")
                (instrument-mod-selected-slot))
              (fx-set-instrument-value
                (dict :idx (get target :depth-idx) :control "param")
                0))))))))

(def instrument-param-mod-bg (p)
  (if (and instrument-mods-open (get p :modulatable))
    (rgba 0.18 0.48 0.95 0.24)
    :transparent))

(def instrument-param-mod-wrapper (p key body)
  (if (and instrument-mods-open (get p :modulatable))
    (subtree :key key
      (box :background-color (instrument-param-mod-bg p)
           :corner-radius 8
           :border-width 1
           :padding 0.08
           :on-double-click (lambda (info) (instrument-toggle-param-modulation p))
        body))
    body))

(def fx-param-row (p fx subtree-key)
  (subtree :key subtree-key
    (instrument-param-mod-wrapper p (str subtree-key "-mod-wrapper")
    (box :height 1.25
      (h-stack :gap 0.45 :align :center
        (box :width 13.2 :height 1.25
          (h-stack :gap 0.25 :align :baseline
            (label (substring (get p :name) 0 9) :font-size 12 :width 7
                   :color :dim :bg :transparent)
            (if (get p :boolean)
              (box :width 5.5 :height 1.25 :align :center
                   :bg :transparent
                   :on-click |x y r|
                     (if fx
                       (fx-toggle-effect-value fx p)
                       (fx-toggle-instrument-value p))
                (label (if (fx-param-on? p) "ON" "OFF")
                       :font-size 11 :width 5.5
                       :color :white :bg :transparent))
              (if (get p :options)
              (dropdown :value (get p :text-value)
                :options (get p :options)
                :on-change (lambda (v)
                  (fx-clear-selected-effect)
                  (if fx
                    (host-command
                      (if (get fx :bus-fx)
                        (if (seq-has-selection?) "set-bus-effect-plock-option" "set-bus-effect-param-option")
                        (if (get fx :midi-fx)
                        (if (seq-has-selection?) "set-midi-fx-plock-option" "set-midi-fx-param-option")
                        (if (seq-has-selection?) "set-effect-plock-option" "set-effect-param-option")))
                      (dict :bus (get fx :bus-idx) :slot-idx (get fx :slot-idx)
                            :param-idx (get p :idx) :label v))
                    (fx-set-instrument-option p v)))
                :width 5.8 :height 1.2 :font-size 11)
              (number-picker :value (fx-param-value p)
                :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 2
                :noui true :font-size 12 :text-color :dim
                :on-change (lambda (v)
                  (if fx
                    (fx-set-effect-value fx p v)
                    (instrument-set-param-control-value p v)))
                :width 5.2 :height 1.1)))))
        (if (or (get p :options) (get p :boolean))
          (label "" :width 7.8 :bg :transparent)
          (hslider :width 7.8 :min (instrument-param-control-min p) :max (instrument-param-control-max p)
                   :value (fx-param-value p)
                   :material (aqua-slider-material)
                   :on-change (lambda (v)
                     (if fx
                       (fx-set-effect-value fx p v)
                       (instrument-set-param-control-value p v))))))))))

(def fx-param-grid (params fx)
  (h-stack :gap 1.5 :padding 0
    (each (chunks (visible-params params) 4) |chunk ci|
      (v-stack :gap 0.25
        (each chunk |p pi|
          (fx-param-row p fx
            (if fx
              (if (get fx :midi-fx)
                (str "midi-fx-slot-" (get fx :slot-idx) "-param-" (get p :idx))
                (if (get fx :bus-fx)
                  (str "bus-fx-slot-" (get fx :bus-idx) "-" (get fx :slot-idx) "-param-" (get p :idx))
                  (str "fx-slot-" (get fx :slot-idx) "-param-" (get p :idx))))
              (str "instrument-tab-" instrument-panel-tab "-chunk-" ci "-param-" (get p :idx)))))))))

(def instrument-mod-base-name (name)
  (if (string-ends-with? name " amt")
    (substring name 0 (- (len name) 4))
    (if (string-ends-with? name " src")
      (substring name 0 (- (len name) 4))
      name)))

(def instrument-mod-source-param (params base)
  (nth (filter |p| (= (get p :name) (str base " src")) params) 0))

(def instrument-mod-amount-params (params)
  (filter |p| (string-ends-with? (get p :name) " amt") params))

(def instrument-mod-row (params amount-p subtree-key)
  (let ((base (instrument-mod-base-name (get amount-p :name)))
        (source-p (instrument-mod-source-param params base)))
    (subtree :key subtree-key
      (box :width 12.6 :height 2.35
           :background-color :instrument-group-bg
           :border-width 1
           :corner-radius 16
           :padding 0.25
        (h-stack :width :fill :gap 0.45 :align :center
          (if source-p
            (dropdown :value (get source-p :text-value)
              :options (get source-p :options)
              :on-change (lambda (v) (fx-set-instrument-option source-p v))
              :width 4.8 :height 1.15 :font-size 10)
            (box :width 5.3 :height 1.15))
          (knob-number :label (substring base 0 12)
            :value (fx-param-value amount-p)
            :min (get amount-p :min) :max (get amount-p :max) :decimals 2
            :font-size 10.5 :label-font-size 9
            :text-color :dim :label-color :dim
            :width 5.2 :height 2.05
            :on-change (lambda (v) (fx-set-instrument-value amount-p v))))))))

(def instrument-mod-grid (params)
  (let ((amounts (instrument-mod-amount-params params)))
    (h-stack :gap 0.45 :padding 0
      (each (chunks amounts 3) |chunk ci|
        (v-stack :gap 0.18
          (each chunk |p pi|
            (instrument-mod-row params p
              (str "instrument-mod-row-" ci "-param-" (get p :idx)))))))))

(def instrument-mod-selector-row (modulator)
  (let ((slot (get modulator :slot))
        (label-text (get modulator :label)))
    (subtree :key (str "instrument-mod-selector-" slot)
      (button label-text
        :width 4.7 :height 1.1
        :padding 0
        :font-size 9
        :background-color (if (= instrument-selected-mod-slot slot)
          (rgba 0.95 0.48 0.18 0.82)
          :instrument-control-bg)
        :color (if (= instrument-selected-mod-slot slot) :white :dim)
        :on-click (lambda (info) (set! instrument-selected-mod-slot slot))))))

(def instrument-mod-selector (inst)
  (box :debug-name "instrument-mod-selector"
       :width 10.2
       :height :fill
       :padding 0.25
    (v-stack :gap 0.18 :align :start
      (label "mods" :font-size 9 :color :dim :bg :transparent)
      (h-stack :gap 0.18 :align :start
        (each (chunks (get inst :modulators) 5) |column ci|
          (v-stack :gap 0.18 :align :start
            (each column |modulator mi|
              (instrument-mod-selector-row modulator))))))))

(def instrument-mod-source-section-name (slot)
  (if (= slot 1) "LFO 1"
    (if (= slot 2) "ENV 1"
      (if (= slot 3) "RAND"
        (if (= slot 4) "DRIFT"
          (if (= slot 5) "LFO 2"
            (if (= slot 6) "LFO 3" false)))))))

(def instrument-selected-mod-source-section (inst)
  (let ((section-name (instrument-mod-source-section-name (instrument-mod-selected-slot))))
    (if section-name
      (nth (filter |section| (= (get section :name) section-name)
             (get inst :sources))
           0)
      false)))

(def instrument-source-param (section name)
  (nth (filter |p| (= (get p :name) name) (get section :params)) 0))

(def instrument-source-param-value (p fallback)
  (if p (fx-param-value p) fallback))

(def instrument-source-set-param-value (p v)
  (if p (instrument-set-param-control-value p v) false))

(def instrument-source-button (p title width)
  (let ((active (> (reactive-value (instrument-source-param-value p 0)) 0.5)))
    (v-stack :width width :height 1.72 :gap 0.10 :align :start
      (label title :font-size 8.2 :width width :height 0.52 :color :dim :bg :transparent)
      (button (if active "ON" "OFF")
        :width width :height 0.88 :padding 0 :font-size 9
        :background-color (if active (ui-accent-orange) :mixer-control-bg)
        :color (if active :black :dim)
        :on-click |x y r|
          (instrument-source-set-param-value p (if active 0 1))))))

(def instrument-source-dropdown (p title width)
  (v-stack :width width :height 1.72 :gap 0.10 :align :start
    (label title :font-size 8.2 :width width :height 0.52 :color :dim :bg :transparent)
    (dropdown :value (if p (get p :text-value) "")
      :options (if p (get p :options) '())
      :on-change (lambda (v) (if p (fx-set-instrument-option p v) false))
      :width width :height 0.88 :font-size 8.5)))

(def instrument-source-number (p title decimals unit width)
  (v-stack :width width :height 1.72 :gap 0.10 :align :start
    (label title :font-size 8.2 :width width :height 0.52 :color :dim :bg :transparent)
    (number-picker :value (instrument-source-param-value p 0)
      :min (if p (instrument-param-control-min p) 0)
      :max (if p (instrument-param-control-max p) 0)
      :decimals decimals
      :unit unit
      :noui true :font-size 9.3
      :text-color :dim :edit-color :yellow
      :text-align :left
      :width width :height 0.82
      :on-change (lambda (v) (instrument-source-set-param-value p v)))))

(def instrument-source-compact-knob (p title decimals)
  (box :debug-name (str "instrument-source-compact-knob-" title)
       :width 4.4 :height 2.25 :padding 0
    (knob-number :label title
      :value (instrument-source-param-value p 0)
      :min (if p (instrument-param-control-min p) 0)
      :max (if p (instrument-param-control-max p) 0)
      :decimals decimals
      :font-size 9.4 :label-font-size 8.2
      :text-color :dim :label-color :dim
      :width 4.4 :height 2.05
      :on-change (lambda (v) (instrument-source-set-param-value p v)))))

(def instrument-source-adsr-number (p title decimals unit)
  (v-stack :width 3.8 :height 1.18 :gap 0.16 :align :start
    (label title :font-size 7.4 :width 3.8 :height 0.52 :color :dim :bg :transparent)
    (number-picker :value (instrument-source-param-value p 0)
      :min (if p (instrument-param-control-min p) 0)
      :max (if p (instrument-param-control-max p) 0)
      :decimals decimals
      :unit unit
      :noui true :font-size 9.0
      :text-color :widget_focus_bg :edit-color :yellow
      :text-align :left
      :width 3.8 :height 0.50
      :on-change (lambda (v) (instrument-source-set-param-value p v)))))

(def instrument-env-source-editor (section)
  (let ((attack (instrument-source-param section "attack"))
        (decay (instrument-source-param section "decay"))
        (sustain (instrument-source-param section "sustain"))
        (release (instrument-source-param section "release")))
    (ui-readout-panel-medium-s 0
      (h-stack :width :fill :height :fill :gap 0.24 :align :stretch
        (adsr-editor
          :attack (instrument-source-param-value attack 5)
          :decay (instrument-source-param-value decay 120)
          :sustain (instrument-source-param-value sustain 0.7)
          :release (instrument-source-param-value release 120)
          :width 13.2 :height :fill
          :background-color :instrument-control-bg
          :on-change (lambda (env)
            (do
              (instrument-source-set-param-value attack (get env :attack))
              (instrument-source-set-param-value decay (get env :decay))
              (instrument-source-set-param-value sustain (get env :sustain))
              (instrument-source-set-param-value release (get env :release)))))
        (v-stack :width 8.2 :height :fill :gap 0.10 :align :start
          (ui-lego-badge-dark "ENV 1" 7.7 (ui-accent-blue))
          (h-stack :gap 0.14 :align :start
            (instrument-source-adsr-number attack "atk" 0 "ms")
            (instrument-source-adsr-number decay "dec" 0 "ms"))
          (h-stack :gap 0.14 :align :start
            (instrument-source-adsr-number sustain "sus" 2 false)
            (instrument-source-adsr-number release "rel" 0 "ms")))))))

(def instrument-lfo-source-editor (section)
  (let ((rate (instrument-source-param section "rate"))
        (sync (instrument-source-param section "sync"))
        (division (instrument-source-param section "division"))
        (shape (instrument-source-param section "shape"))
        (pulse-width (instrument-source-param section "pulse width"))
        (retrigger (instrument-source-param section "retrigger")))
    (ui-readout-panel-medium-s 0
      (h-stack :debug-name "instrument-lfo-source-editor"
               :width :fill :height :fill :gap 0.38 :align :start
        (v-stack :width 13.8 :height :fill :gap 0.12 :align :start
          (h-stack :gap 0.25 :align :start
            (instrument-source-number rate "rate" 2 false 6.4)
            (instrument-source-button sync "sync" 5.0))
          (h-stack :gap 0.25 :align :start
            (instrument-source-dropdown division "division" 6.4)
            (instrument-source-dropdown shape "shape" 5.0)))
        (v-stack :width 5.0 :height :fill :gap 0.18 :align :center
          (instrument-source-compact-knob pulse-width "pw" 2)
          (box :debug-name "instrument-lfo-retrigger-button"
               :width 4.4 :height 1.55 :padding 0
            (instrument-source-button retrigger "retrig" 4.4)))))))

(def instrument-lfo-source-section? (section)
  (or (= (get section :name) "LFO 1")
      (= (get section :name) "LFO 2")
      (= (get section :name) "LFO 3")))

(def instrument-selected-mod-source-editor (inst)
  (let ((slot (instrument-mod-selected-slot)))
    (box :debug-name "instrument-selected-mod-source-editor"
         :width 25.5
         :height :fill
         :padding 0.35
      (if (> slot 6)
        (box :width :fill :height :fill :h-align :center :v-align :center
          (label "external mod" :font-size 12 :color :dim :bg :transparent))
        (let ((section (instrument-selected-mod-source-section inst)))
          (if section
            (v-stack :width :fill :height :fill :gap 0.3 :align :start
              (label (get section :name) :font-size 9 :color :dim :bg :transparent)
              (if (= (get section :name) "ENV 1")
                (instrument-env-source-editor section)
                (if (instrument-lfo-source-section? section)
                  (instrument-lfo-source-editor section)
                  (fx-param-grid (get section :params) false))))
            (box :width :fill :height :fill :h-align :center :v-align :center
              (label "no source controls" :font-size 12 :color :dim :bg :transparent))))))))

(def instrument-mod-control-panel (inst)
  (box :debug-name "instrument-mod-control-panel"
       :width 36.4
       :height :fill
       :background-color :black
       :corner-radius 10
       :padding 0.25
    (h-stack :height :fill :gap 0.25 :align :stretch
      (instrument-mod-selector inst)
      (instrument-selected-mod-source-editor inst))))

(def instrument-sources-grid (sections)
  (h-stack :gap 2 
    (each sections |section si|
      (v-stack :gap 0.25
        (label (get section :name) :font-size 14 :color :white :bg :transparent)
        (each (get section :params) |p pi|
          (fx-param-row p false
            (str "instrument-source-" si "-param-" (get p :idx))))))))

(def instrument-source-tabs (inst)
  (if (> (len (get inst :sources)) 0)
    (tabs :items (get inst :source-names)
      :bind instrument-source-tab
      :compact true
      :gap 0.75
      :tab-padding 0.5
      :header-height 1
      (each (get inst :sources) |section si|
        (fx-param-grid (get section :params) false)))
    (instrument-sources-grid (get inst :sources))))

(defwidget header
  :shader
  (rgba 1 1 1 1))

(def enabled-param (params)
  (nth (filter |p| (= (get p :name) "enabled") params) 0))

(def visible-params (params)
  (filter |p| (not (= (get p :name) "enabled")) params))

(load "metal-seq-builtin-fx-ui.lisp")

(defwidget fx-enabled-dot
  :width 1.55 :height 1.0
  :paint-margin 0.1
  :state (active)
  :bindable (active)
  :shader
  (sdf/fill (sdf/circle 0.86)
    (material :color (if (> active 0.5) (rgba 1.0 0.8 0.12 1.0) (rgba 0 0 0 1)))))

(def fx-enabled-toggle (p fx subtree-key)
  (subtree :key subtree-key
    (box :width 1.55 :height 1.35 :v-align :start :h-align :center :padding 0
      (v-stack :gap 0 :align :center
        (box :width 1.55 :height 0.14)
        (if p
          (fx-enabled-dot
            :active (fx-param-value p)
            :on-click |x y r|
              (if fx
                (fx-toggle-effect-value fx p)
                (fx-toggle-instrument-value p)))
          (box :width 1.55 :height 1.0))))))

(defwidget fx-mini-save-icon
  :width 1.5 :height 0.8
  :paint-margin 0.2
  :state (active)
  :shader
  (let ((fg-col (rgba 0.92 0.92 0.96 1.0))
        (bg-col (if (= active 1)
          (rgba 0.00 0.35 0.82 1.0)
          (rgba 0.28 0.28 0.30 1.0))))
    (sdf/layer
      (sdf/fill
        (sdf/rounded-rect width height 0.5)
        (material :color bg-col))
      (sdf/fill
        (sdf/translate 0.0 -0.42
          (sdf/rounded-rect 0.30 0.20 0.08))
        (material :color fg-col))
      (sdf/fill
        (sdf/translate 0.16 -0.42
          (sdf/rounded-rect 0.10 0.16 0.06))
        (material :color bg-col))
      (sdf/fill
        (sdf/translate 0.0 0.27
          (sdf/rounded-rect 0.34 0.22 0.08))
        (material :color fg-col)))))

(def fx-panel (title params fx)
  (let ((selected (fx-panel-selected? fx)))
  (box
    (v-stack :gap 0
      (fx-panel-header title params fx)
      (fx-panel-body (if (get fx :midi-fx) "midi-fx-panel-content" "audio-fx-panel-content")
        (if (get fx :midi-fx)
          (midi-fx-panel-body fx)
          (audio-fx-panel-body fx params))))
    :background "fx-panel-bg"
    :color :fx-panel-bg
    :header :fx-panel-header-bg
    :selected-header :fx-panel-header-selected-bg
    :height fx-fixed-panel-height
    :debug-name (if (get fx :midi-fx)
      (str "midi-fx-panel-root-" (get fx :slot-idx) "-" (get fx :name))
      (if (get fx :bus-fx)
        (str "bus-fx-panel-root-" (get fx :bus-idx) "-" (get fx :slot-idx) "-" title)
        (str "audio-fx-panel-root-" (get fx :slot-idx) "-" title)))
    :drop-types (fx-effect-drop-types fx)
    :drop-meta (fx-effect-drop-meta fx)
    :drop-hover-border-color :blue
    :on-drop (lambda (event) (fx-drop-on-effect event))
    :selected (if selected 1 0)
    :padding 0)))

(def midi-fx-panel (title params fx)
  (let ((selected (= selected-midi-fx-slot (get fx :slot-idx))))
  (box
    (v-stack :gap 0
      (fx-panel-header title params fx)
      (fx-panel-body "midi-fx-panel-content"
        (subtree :key (str "midi-fx-panel-body-" (get fx :slot-idx) "-" (get fx :name))
          (midi-fx-panel-body fx))))
    :background "fx-panel-bg"
    :color :fx-panel-bg
    :header :fx-panel-header-bg
    :selected-header :fx-panel-header-selected-bg
    :height fx-fixed-panel-height
    :debug-name (str "midi-fx-panel-bg-" (get fx :slot-idx) "-" (get fx :name))
    :drop-types (fx-effect-drop-types fx)
    :drop-meta (fx-effect-drop-meta fx)
    :drop-hover-border-color :blue
    :on-drop (lambda (event) (fx-drop-on-effect event))
    :selected (if selected 1 0)
    :padding 0)))

(def instrument-tab-button (text idx width)
  (box :width width :height 1.2 :align :center
    :bg (if (= instrument-panel-tab idx) :dark-gray :transparent)
    :on-click |x y r| (set! instrument-panel-tab idx)
    (label text :font-size 11
      :color (if (= instrument-panel-tab idx) :white :dim)
      :bg :transparent)))

(def instrument-header-button (text active width click)
  (box :width width :height 1.2 :align :center
    :bg (if active :dark-gray :transparent)
    :on-click click
    (label text :font-size 11
      :color (if active :white :dim)
      :bg :transparent)))

(def instrument-synth-button ()
  (instrument-header-button "synth" (and (= instrument-panel-tab 0) (not instrument-mods-open)) 4.5
    (lambda (info) (do (set! instrument-panel-tab 0) (set! instrument-mods-open false)))))

(def instrument-mods-toggle-button ()
  (instrument-header-button "mods" (and (= instrument-panel-tab 0) instrument-mods-open) 4.0
    (lambda (info) (do (set! instrument-panel-tab 0) (set! instrument-mods-open (not instrument-mods-open))))))

(def inst-param (inst name)
  (nth (filter |p| (= (get p :name) name) (get inst :synth)) 0))

(def inst-base-note-param (inst)
  (nth (filter |p| (= (get p :control) "base-note") (get inst :synth)) 0))

(def inst-param-row (inst name key)
  (let ((p (inst-param inst name)))
    (if p
      (fx-param-row p false key)
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def ui-param-control (name)
  (let ((p (inst-param synth-ui-current-inst name)))
    (if p
      (fx-param-row p false (str "custom-ui-" synth-ui-current-name "-" name))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def custom-ui-scope-name ()
  (if (= custom-ui-current-kind "audio-fx")
    (if (get audio-fx-ui-current-fx :bus-fx)
      (str audio-fx-ui-current-name "-bus-" (get audio-fx-ui-current-fx :bus-idx)
           "-slot-" (get audio-fx-ui-current-fx :slot-idx))
      (str audio-fx-ui-current-name "-slot-" (get audio-fx-ui-current-fx :slot-idx)))
    synth-ui-current-name))

(def custom-ui-current-scope ()
  (dict
    :kind custom-ui-current-kind
    :name (custom-ui-scope-name)
    :audio-fx audio-fx-ui-current-fx
    :inst synth-ui-current-inst))

(def custom-ui-param-in-scope (scope name)
  (if (= (get scope :kind) "audio-fx")
    (audio-fx-ui-param (get scope :audio-fx) name)
    (inst-param (get scope :inst) name)))

(def custom-ui-base-note-param-in-scope (scope)
  (if (= (get scope :kind) "audio-fx")
    false
    (inst-base-note-param (get scope :inst))))

(def custom-ui-set-param-in-scope (scope p value)
  (if (= (get scope :kind) "audio-fx")
    (fx-set-effect-value (get scope :audio-fx) p value)
    (instrument-set-param-control-value p value)))

(def custom-ui-set-param-by-name-in-scope (scope name value)
  (let ((p (custom-ui-param-in-scope scope name)))
    (if p (custom-ui-set-param-in-scope scope p value) false)))

(def custom-ui-param-change-callback (p)
  (let ((scope (custom-ui-current-scope)))
    (lambda (v)
      (if (= (get scope :kind) "audio-fx")
        (custom-ui-set-param-in-scope scope p v)
        (instrument-set-param-control-value p v)))))

(def custom-ui-param-change-callback-s (section p)
  (let ((scope (custom-ui-current-scope)))
    (lambda (v)
      (do
        (custom-ui-select-section-in-scope scope section)
        (if (= (get scope :kind) "audio-fx")
          (custom-ui-set-param-in-scope scope p v)
          (instrument-set-param-control-value p v))))))

(def custom-ui-current-param (name)
  (if (= custom-ui-current-kind "audio-fx")
    (audio-fx-ui-param audio-fx-ui-current-fx name)
    (inst-param synth-ui-current-inst name)))

(def custom-ui-current-base-note-param ()
  (if (= custom-ui-current-kind "audio-fx")
    false
    (inst-base-note-param synth-ui-current-inst)))

(def custom-ui-set-param (p value)
  (if (= custom-ui-current-kind "audio-fx")
    (custom-ui-set-param-in-scope (custom-ui-current-scope) p value)
    (instrument-set-param-control-value p value)))

(def custom-ui-set-param-by-name (name value)
  (let ((p (custom-ui-current-param name)))
    (if p (custom-ui-set-param p value) false)))

(def base-note ()
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (subtree :key (str "custom-ui-base-note-" synth-ui-current-name)
        (knob-number :label "note"
          :value (fx-param-value p)
          :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 0
          :step 1
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width 4.4 :height 2.4
          :value-align :center
          :on-change (lambda (v) (instrument-set-param-control-value p v))))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))

(defstate custom-ui-selected-sections '())
(def custom-ui-selected-section 0)

(def custom-ui-selected-section-for-scope (scope-name)
  (let ((entry
          (nth
            (filter |item| (= (get item :scope) scope-name)
              custom-ui-selected-sections)
            0)))
    (if entry (get entry :section) 0)))

(def custom-ui-selected-section-for-current-scope ()
  (custom-ui-selected-section-for-scope (custom-ui-scope-name)))

(def custom-ui-set-selected-section-for-scope (scope-name section)
  (set! custom-ui-selected-sections
    (cons
      (dict :scope scope-name :section section)
      (filter |item| (not (= (get item :scope) scope-name))
        custom-ui-selected-sections))))

(def custom-ui-select-section-in-scope (scope section)
  (custom-ui-set-selected-section-for-scope (get scope :name) section))

(def ui-select-section (section)
  (custom-ui-set-selected-section-for-scope (custom-ui-scope-name) section))

(def ui-section-select-callback (section)
  (let ((scope-name (custom-ui-scope-name)))
    (lambda (info)
      (custom-ui-set-selected-section-for-scope scope-name section))))

(def ui-panel-bg (section)
  (if (= section 0)
    :instrument-group-bg
    (if (= custom-ui-selected-section section)
      :instrument-group-selected-bg
      :instrument-group-bg)))

(def ui-row-label (title)
  (box :width 3.0 :height 2.1 :h-align :center :v-align :center :padding 0.1
    (label title :font-size 8.0 :width 2.7 :color :dim :bg :transparent)))

(def ui-panel-header (title)
  (box :width :fill :height 0.5 :h-align :start :v-align :center :padding 0.15
    (label title :font-size 7.5 :color :dim :bg :transparent)))

(def ui-section (title body)
  (box :width :fill :height 3.4
       :background-color :instrument-group-bg
       :border-width 1 :corner-radius 12 :padding 0.15
    (v-stack :width :fill :gap 0.2 :align :start
      (ui-panel-header title)
      body)))

(def ui-panel (title section body)
  (box :width :fill :height 3.4
       :background-color (ui-panel-bg section)
       :border-width 1 :corner-radius 12 :padding 0.15
       :on-click (ui-section-select-callback section)
    (v-stack :width :fill :gap 0.2 :align :start
      (ui-panel-header title)
      body)))

(def ui-param-knob (name title)
  (let ((p (custom-ui-current-param name)))
    (if p
      (instrument-param-mod-wrapper p (str "custom-ui-knob-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-knob-" (custom-ui-scope-name) (instrument-param-control-key-mode p) "-" name)
          (knob-number :label title
            :value (fx-param-value p)
            :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 2
            :base-value (instrument-param-base-value-prop p)
            :base-min (instrument-param-base-min-prop p) :base-max (instrument-param-base-max-prop p)
            :mod-range-0-slot (instrument-param-knob-mod-slot-prop p 0) :mod-range-0-depth (instrument-param-knob-mod-depth-prop p 0)
            :mod-range-1-slot (instrument-param-knob-mod-slot-prop p 1) :mod-range-1-depth (instrument-param-knob-mod-depth-prop p 1)
            :mod-range-2-slot (instrument-param-knob-mod-slot-prop p 2) :mod-range-2-depth (instrument-param-knob-mod-depth-prop p 2)
            :mod-range-3-slot (instrument-param-knob-mod-slot-prop p 3) :mod-range-3-depth (instrument-param-knob-mod-depth-prop p 3)
            :mod-range-4-slot (instrument-param-knob-mod-slot-prop p 4) :mod-range-4-depth (instrument-param-knob-mod-depth-prop p 4)
            :mod-range-5-slot (instrument-param-knob-mod-slot-prop p 5) :mod-range-5-depth (instrument-param-knob-mod-depth-prop p 5)
            :mod-range-6-slot (instrument-param-knob-mod-slot-prop p 6) :mod-range-6-depth (instrument-param-knob-mod-depth-prop p 6)
            :mod-range-7-slot (instrument-param-knob-mod-slot-prop p 7) :mod-range-7-depth (instrument-param-knob-mod-depth-prop p 7)
            :mod-range-8-slot (instrument-param-knob-mod-slot-prop p 8) :mod-range-8-depth (instrument-param-knob-mod-depth-prop p 8)
            :mod-range-9-slot (instrument-param-knob-mod-slot-prop p 9) :mod-range-9-depth (instrument-param-knob-mod-depth-prop p 9)
            :selected-mod-slot (instrument-selected-mod-slot-prop p)
            :font-size 10.5 :label-font-size 10
            :text-color :dim :label-color :dim
            :width 4.4 :height 2.4
            :value-align :center
            :on-change (custom-ui-param-change-callback p))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

;; Compact knob: ~1.7 cell tall, value nestled in the lower-right of the knob
;; arc (default value-align) so the knob itself stays large. For instruments
;; that need 3-4 rows of params instead of 2.
(def ui-param-knob-c (name title)
  (let ((p (custom-ui-current-param name)))
    (if p
      (instrument-param-mod-wrapper p (str "custom-ui-knob-c-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-knob-c-" (custom-ui-scope-name) (instrument-param-control-key-mode p) "-" name)
          (knob-number :label title
            :value (fx-param-value p)
            :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 2
            :base-value (instrument-param-base-value-prop p)
            :base-min (instrument-param-base-min-prop p) :base-max (instrument-param-base-max-prop p)
            :mod-range-0-slot (instrument-param-knob-mod-slot-prop p 0) :mod-range-0-depth (instrument-param-knob-mod-depth-prop p 0)
            :mod-range-1-slot (instrument-param-knob-mod-slot-prop p 1) :mod-range-1-depth (instrument-param-knob-mod-depth-prop p 1)
            :mod-range-2-slot (instrument-param-knob-mod-slot-prop p 2) :mod-range-2-depth (instrument-param-knob-mod-depth-prop p 2)
            :mod-range-3-slot (instrument-param-knob-mod-slot-prop p 3) :mod-range-3-depth (instrument-param-knob-mod-depth-prop p 3)
            :mod-range-4-slot (instrument-param-knob-mod-slot-prop p 4) :mod-range-4-depth (instrument-param-knob-mod-depth-prop p 4)
            :mod-range-5-slot (instrument-param-knob-mod-slot-prop p 5) :mod-range-5-depth (instrument-param-knob-mod-depth-prop p 5)
            :mod-range-6-slot (instrument-param-knob-mod-slot-prop p 6) :mod-range-6-depth (instrument-param-knob-mod-depth-prop p 6)
            :mod-range-7-slot (instrument-param-knob-mod-slot-prop p 7) :mod-range-7-depth (instrument-param-knob-mod-depth-prop p 7)
            :mod-range-8-slot (instrument-param-knob-mod-slot-prop p 8) :mod-range-8-depth (instrument-param-knob-mod-depth-prop p 8)
            :mod-range-9-slot (instrument-param-knob-mod-slot-prop p 9) :mod-range-9-depth (instrument-param-knob-mod-depth-prop p 9)
            :selected-mod-slot (instrument-selected-mod-slot-prop p)
            :font-size 8.5 :label-font-size 7.5
            :text-color :dim :label-color :dim
            :width 3.8 :height 1.8
            :label-height 0.5 :knob-size 1.25
            :on-change (custom-ui-param-change-callback p))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def base-note-c ()
  (let ((p (custom-ui-current-base-note-param)))
    (if p
      (subtree :key (str "custom-ui-base-note-c-" (custom-ui-scope-name))
        (knob-number :label "note"
          :value (fx-param-value p)
          :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 0
          :step 1
          :font-size 8.5 :label-font-size 7.5
          :text-color :dim :label-color :dim
          :width 3.8 :height 1.8
          :label-height 0.5 :knob-size 1.25
          :on-change (custom-ui-param-change-callback p)))
      (label "missing: base_note" :font-size 9 :color :red :bg :transparent))))

(def ui-panel-header-c (title)
  (box :width 3.5 :height :fill :h-align :end :v-align :center :padding 0.1
    (label title :font-size 6 :color :dim :bg :transparent)))

;; Compact panel: title runs along the LEFT edge (vertical strip) so each
;; row only takes the height of one knob — no separate title band on top.
(def ui-panel-c (title section body)
  (box :width :fill :height 2.0
       :background-color (ui-panel-bg section)
       :border-width 1 :corner-radius 10 :padding 0.08
       :on-click (ui-section-select-callback section)
    (h-stack :width :fill :gap 0.1 :align :center
      (ui-panel-header-c title)
      body)))

(def ui-param-value (name fallback)
  (let ((p (custom-ui-current-param name)))
    (if p (get p :value) fallback)))

(def ui-param-bound-value (name fallback)
  (let ((p (custom-ui-current-param name)))
    (if p (fx-param-value p) fallback)))

(def ui-set-param (name value)
  (let ((p (custom-ui-current-param name)))
    (if p (custom-ui-set-param p value) false)))

(def ui-accent-blue () (rgba 0.00 0.48 0.95 1.0))
(def ui-accent-cyan () (rgba 0.05 0.78 0.90 1.0))
(def ui-accent-orange () (rgba 1.0 0.62 0.25 1.0))
(def ui-accent-green () (rgba 0.30 0.82 0.48 1.0))
(def ui-accent-violet () (rgba 0.62 0.45 0.95 1.0))

(def ui-lego-gap () 0.25)
(def ui-lego-small-h () 1.95)
(def ui-lego-medium-h () 4.08)
(def ui-lego-dense-h () 3.08)
(def ui-lego-full-h ()
  (+ (ui-lego-medium-h) (ui-lego-small-h) (ui-lego-small-h)
     (ui-lego-gap) (ui-lego-gap)))
(def ui-lego-col-w () 24.0)
(def ui-lego-strip-w () 7.2)

(def ui-lego-title (title accent)
  (box :width :fill :height 0.48 :h-align :start :v-align :center :padding 0.08
    (label title :font-size 8.6 :color :dim :bg :transparent)))

(def ui-lego-surface (title height accent surface body)
  (box :width (ui-lego-col-w) :height height
       :background-color surface
       :corner-radius 7
       :border-width 1
       :padding 0.24
    (v-stack :width :fill :height :fill :gap 0.18
      (ui-lego-title title accent)
      (box :width :fill :flex 1 :padding 0.12 body))))

(def ui-lego-surface-s (title height accent section surface body)
  (box :width (ui-lego-col-w) :height height
       :background-color (if (= surface :instrument-group-bg) (ui-panel-bg section) surface)
       :corner-radius 7
       :border-width 1
       :padding 0.24
       :on-click (ui-section-select-callback section)
    (v-stack :width :fill :height :fill :gap 0.18
      (ui-lego-title title accent)
      (box :width :fill :flex 1 :padding 0.12 body))))

(def ui-lego-surface-width-s (title width height accent section surface body)
  (box :width width :height height
       :background-color (if (= surface :instrument-group-bg) (ui-panel-bg section) surface)
       :corner-radius 7
       :border-width 1
       :padding 0.20
       :on-click (ui-section-select-callback section)
    (v-stack :width :fill :height :fill :gap 0.12
      (ui-lego-title title accent)
      (box :width :fill :flex 1 :padding 0.06 body))))

(def ui-lego-panel-s (height section surface body)
  (box :width (ui-lego-col-w) :height height
       :background-color (if (= surface :instrument-group-bg) (ui-panel-bg section) surface)
       :corner-radius 7
       :border-width 1
       :padding 0.18
       :on-click (ui-section-select-callback section)
    (box :width :fill :height :fill :padding 0.04 body)))

(def ui-lego-panel-width-s (width height section surface body)
  (box :width width :height height
       :background-color (if (= surface :instrument-group-bg) (ui-panel-bg section) surface)
       :corner-radius 7
       :border-width 1
       :padding 0.18
       :on-click (ui-section-select-callback section)
    (box :width :fill :height :fill :padding 0.04 body)))

(def ui-lego-plain-surface (height surface body)
  (box :width (ui-lego-col-w) :height height
       :background-color surface
       :corner-radius 7
       :border-width 1
       :padding 0.16
       :debug-name "ui-lego-plain-surface"
       :v-align :center
    (box :width :fill :padding 0.12
      (h-stack :width :fill :gap 0 :align :center
        (box :width 0.55 :height 0.1)
        body))))

(def ui-lego-plain-surface-s (height section surface body)
  (box :width (ui-lego-col-w) :height height
       :background-color surface
       :corner-radius 7
       :border-width 1
       :padding 0.16
       :debug-name "ui-lego-plain-surface"
       :v-align :center
       :on-click (ui-section-select-callback section)
    (box :width :fill :padding 0.12
      (h-stack :width :fill :gap 0 :align :center
        (box :width 0.55 :height 0.1)
        body))))

(def ui-lego-text-row-3 (a b c)
  (box :width :fill :height 1.28 :v-align :start :debug-name "ui-lego-text-row"
    (h-stack :gap 0.34 :align :start a b c)))

(def ui-lego-text-row-4 (a b c d)
  (box :width :fill :height 1.28 :v-align :start :debug-name "ui-lego-text-row"
    (h-stack :gap 0.34 :align :start a b c d)))

(def ui-control-block-small (title accent body)
  (ui-lego-surface title (ui-lego-small-h) accent :instrument-group-bg body))

(def ui-control-block-medium (title accent body)
  (ui-lego-surface title (ui-lego-medium-h) accent :instrument-group-bg body))

(def ui-control-block-full (title accent body)
  (ui-lego-surface title (ui-lego-full-h) accent :instrument-group-bg body))

(def ui-control-block-small-s (title accent section body)
  (ui-lego-surface-s title (ui-lego-small-h) accent section :instrument-group-bg body))

(def ui-control-block-medium-s (title accent section body)
  (ui-lego-surface-s title (ui-lego-medium-h) accent section :instrument-group-bg body))

(def ui-control-block-dense-s (title accent section body)
  (ui-lego-surface-s title (ui-lego-dense-h) accent section :instrument-group-bg body))

(def ui-control-panel-dense-s (section body)
  (ui-lego-panel-s (ui-lego-dense-h) section :instrument-group-bg body))

(def ui-control-panel-small-s (section body)
  (ui-lego-panel-s (ui-lego-small-h) section :instrument-group-bg body))

(def ui-control-panel-medium-s (section body)
  (ui-lego-panel-s (ui-lego-medium-h) section :instrument-group-bg body))

(def ui-control-block-full-s (title accent section body)
  (ui-lego-surface-s title (ui-lego-full-h) accent section :instrument-group-bg body))

(def ui-readout-block-small (title accent body)
  (ui-lego-plain-surface (ui-lego-small-h) (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-block-small-s (title accent section body)
  (ui-lego-plain-surface-s (ui-lego-small-h) section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-block-dense-s (title accent section body)
  (ui-lego-surface-s title (ui-lego-dense-h) accent section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-panel-small-s (section body)
  (ui-lego-panel-s (ui-lego-small-h) section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-panel-dense-s (section body)
  (ui-lego-panel-s (ui-lego-dense-h) section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-panel-medium-s (section body)
  (ui-lego-panel-s (ui-lego-medium-h) section (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-block-medium (title accent body)
  (ui-lego-surface title (ui-lego-medium-h) accent (rgba 0.055 0.058 0.064 1.0) body))

(def ui-readout-block-full (title accent body)
  (ui-lego-surface title (ui-lego-full-h) accent (rgba 0.055 0.058 0.064 1.0) body))

(def ui-lego-column (a b c)
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap) a b c))

(def ui-lego-column-2 (a b)
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap) a b))

(def ui-lego-column-full (a)
  (v-stack :width (ui-lego-col-w) :gap (ui-lego-gap) a))

(def ui-lego-strip-s (title accent section body)
  (ui-lego-surface-width-s title (ui-lego-strip-w) (ui-lego-full-h) accent section :instrument-group-bg body))

(def ui-lego-strip-half-s (title accent section body)
  (ui-lego-surface-width-s title (ui-lego-strip-w) (ui-lego-medium-h) accent section :instrument-group-bg body))

(def ui-lego-strip-panel-s (section body)
  (ui-lego-panel-width-s (ui-lego-strip-w) (ui-lego-full-h) section :instrument-group-bg body))

(def ui-lego-badge (title width accent)
  (box :width width :height 1.18 :v-align :end
    (badge title
      :width width :height 0.82 :padding 0 :font-size 9.2
      :variant :secondary
      :color accent)))

(def ui-lego-badge-s (section title width accent)
  (box :width width :height 1.18 :v-align :end
    (badge title
      :width width :height 0.82 :padding 0 :font-size 9.2
      :variant :secondary
      :color accent)))

(def ui-lego-badge-dark (title width accent)
  (box :width width :height 1.18 :v-align :end
    (badge title
      :width width :height 0.82 :padding 0 :font-size 9.2
      :background-color :instrument-control-bg
      :color accent)))

(def ui-lego-knob (name title width accent decimals)
  (let ((p (custom-ui-current-param name)))
    (if p
      (instrument-param-mod-wrapper p (str "custom-ui-lego-knob-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-knob-" (custom-ui-scope-name) (instrument-param-control-key-mode p) "-" name)
          (knob-number :label title
            :value (fx-param-value p)
            :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals decimals
            :base-value (instrument-param-base-value-prop p)
            :base-min (instrument-param-base-min-prop p) :base-max (instrument-param-base-max-prop p)
            :mod-range-0-slot (instrument-param-knob-mod-slot-prop p 0) :mod-range-0-depth (instrument-param-knob-mod-depth-prop p 0)
            :mod-range-1-slot (instrument-param-knob-mod-slot-prop p 1) :mod-range-1-depth (instrument-param-knob-mod-depth-prop p 1)
            :mod-range-2-slot (instrument-param-knob-mod-slot-prop p 2) :mod-range-2-depth (instrument-param-knob-mod-depth-prop p 2)
            :mod-range-3-slot (instrument-param-knob-mod-slot-prop p 3) :mod-range-3-depth (instrument-param-knob-mod-depth-prop p 3)
            :mod-range-4-slot (instrument-param-knob-mod-slot-prop p 4) :mod-range-4-depth (instrument-param-knob-mod-depth-prop p 4)
            :mod-range-5-slot (instrument-param-knob-mod-slot-prop p 5) :mod-range-5-depth (instrument-param-knob-mod-depth-prop p 5)
            :mod-range-6-slot (instrument-param-knob-mod-slot-prop p 6) :mod-range-6-depth (instrument-param-knob-mod-depth-prop p 6)
            :mod-range-7-slot (instrument-param-knob-mod-slot-prop p 7) :mod-range-7-depth (instrument-param-knob-mod-depth-prop p 7)
            :mod-range-8-slot (instrument-param-knob-mod-slot-prop p 8) :mod-range-8-depth (instrument-param-knob-mod-depth-prop p 8)
            :mod-range-9-slot (instrument-param-knob-mod-slot-prop p 9) :mod-range-9-depth (instrument-param-knob-mod-depth-prop p 9)
            :selected-mod-slot (instrument-selected-mod-slot-prop p)
            :font-size 10.8 :label-font-size 9.6
            :text-color accent :label-color :dim
            :width width :height 2.62
            :value-align :center
            :on-change (custom-ui-param-change-callback p))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def ui-lego-knob-s (section name title width accent decimals)
  (let ((p (custom-ui-current-param name)))
    (if p
      (instrument-param-mod-wrapper p (str "custom-ui-lego-knob-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-knob-" (custom-ui-scope-name) (instrument-param-control-key-mode p) "-" name)
          (knob-number :label title
            :value (fx-param-value p)
            :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals decimals
            :base-value (instrument-param-base-value-prop p)
            :base-min (instrument-param-base-min-prop p) :base-max (instrument-param-base-max-prop p)
            :mod-range-0-slot (instrument-param-knob-mod-slot-prop p 0) :mod-range-0-depth (instrument-param-knob-mod-depth-prop p 0)
            :mod-range-1-slot (instrument-param-knob-mod-slot-prop p 1) :mod-range-1-depth (instrument-param-knob-mod-depth-prop p 1)
            :mod-range-2-slot (instrument-param-knob-mod-slot-prop p 2) :mod-range-2-depth (instrument-param-knob-mod-depth-prop p 2)
            :mod-range-3-slot (instrument-param-knob-mod-slot-prop p 3) :mod-range-3-depth (instrument-param-knob-mod-depth-prop p 3)
            :mod-range-4-slot (instrument-param-knob-mod-slot-prop p 4) :mod-range-4-depth (instrument-param-knob-mod-depth-prop p 4)
            :mod-range-5-slot (instrument-param-knob-mod-slot-prop p 5) :mod-range-5-depth (instrument-param-knob-mod-depth-prop p 5)
            :mod-range-6-slot (instrument-param-knob-mod-slot-prop p 6) :mod-range-6-depth (instrument-param-knob-mod-depth-prop p 6)
            :mod-range-7-slot (instrument-param-knob-mod-slot-prop p 7) :mod-range-7-depth (instrument-param-knob-mod-depth-prop p 7)
            :mod-range-8-slot (instrument-param-knob-mod-slot-prop p 8) :mod-range-8-depth (instrument-param-knob-mod-depth-prop p 8)
            :mod-range-9-slot (instrument-param-knob-mod-slot-prop p 9) :mod-range-9-depth (instrument-param-knob-mod-depth-prop p 9)
            :selected-mod-slot (instrument-selected-mod-slot-prop p)
            :font-size 10.8 :label-font-size 9.6
            :text-color accent :label-color :dim
            :width width :height 2.62
            :value-align :center
            :on-change (custom-ui-param-change-callback-s section p))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def ui-lego-num (name title width decimals unit accent)
  (let ((p (custom-ui-current-param name)))
    (if p
      (instrument-param-mod-wrapper p (str "custom-ui-lego-num-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-num-" (custom-ui-scope-name) (instrument-param-control-key-mode p) "-" name)
          (v-stack :width width :height 1.12 :gap 0.08 :align :start
            (label title :font-size 8.2 :width width :color :dim :bg :transparent)
            (number-picker :value (fx-param-value p)
              :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals decimals
              :unit unit
              :noui true :font-size 10.2
              :text-color accent :edit-color :yellow
              :text-align :left
              :width width :height 0.68
              :on-change (custom-ui-param-change-callback p)))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def ui-lego-num-s (section name title width decimals unit accent)
  (let ((p (custom-ui-current-param name)))
    (if p
      (instrument-param-mod-wrapper p (str "custom-ui-lego-num-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-num-" (custom-ui-scope-name) (instrument-param-control-key-mode p) "-" name)
          (v-stack :width width :height 1.12 :gap 0.08 :align :start
            (label title :font-size 8.2 :width width :color :dim :bg :transparent)
            (number-picker :value (fx-param-value p)
              :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals decimals
              :unit unit
              :noui true :font-size 10.2
              :text-color accent :edit-color :yellow
              :text-align :left
              :width width :height 0.68
              :on-change (custom-ui-param-change-callback-s section p)))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def ui-lego-micro-num-s (section name title width decimals unit accent)
  (let ((p (custom-ui-current-param name)))
    (if p
      (instrument-param-mod-wrapper p (str "custom-ui-lego-micro-num-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-micro-num-" (custom-ui-scope-name) (instrument-param-control-key-mode p) "-" name)
          (v-stack :width width :height 1.18 :gap 0.16 :align :start
            (label title :font-size 7.4 :width width :height 0.52 :color :dim :bg :transparent)
            (number-picker :value (fx-param-value p)
              :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals decimals
              :unit unit
              :noui true :font-size 9.0
              :text-color accent :edit-color :yellow
              :text-align :left
              :width width :height 0.50
              :on-change (custom-ui-param-change-callback-s section p)))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

(def ui-lego-option (name title width options accent)
  (let ((p (custom-ui-current-param name))
        (scope (custom-ui-current-scope)))
    (if p
      (instrument-param-mod-wrapper p (str "custom-ui-lego-option-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-option-" (custom-ui-scope-name) "-" name)
          (v-stack :width width :height 1.12 :gap 0.08 :align :start
            (label title :font-size 8.2 :width width :color :dim :bg :transparent)
            (dropdown :value-index (fx-param-value p)
              :value-index-offset (get p :min)
              :options options
              :width width :height 0.78 :font-size 8.0
              :on-change (lambda (v)
                (custom-ui-set-param-in-scope
                  scope
                  p
                  (+ (get p :min) (custom-ui-option-index options v))))))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def ui-lego-option-s (section name title width options accent)
  (let ((p (custom-ui-current-param name))
        (scope (custom-ui-current-scope)))
    (if p
      (instrument-param-mod-wrapper p (str "custom-ui-lego-option-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-option-" (custom-ui-scope-name) "-" name)
          (v-stack :width width :height 1.12 :gap 0.08 :align :start
            (label title :font-size 8.2 :width width :color :dim :bg :transparent)
            (dropdown :value-index (fx-param-value p)
              :value-index-offset (get p :min)
              :options options
              :width width :height 0.78 :font-size 8.0
              :on-change (lambda (v)
                (do
                  (custom-ui-select-section-in-scope scope section)
                  (custom-ui-set-param-in-scope
                    scope
                    p
                    (+ (get p :min) (custom-ui-option-index options v)))))))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def ui-lego-micro-option-s (section name title width options accent)
  (let ((p (custom-ui-current-param name))
        (scope (custom-ui-current-scope)))
    (if p
      (instrument-param-mod-wrapper p (str "custom-ui-lego-micro-option-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-micro-option-" (custom-ui-scope-name) "-" name)
          (box :width width :height 1.18 :v-align :end
            (dropdown :value-index (fx-param-value p)
              :value-index-offset (get p :min)
              :options options
              :width width :height 0.92 :font-size 8.6
              :on-change (lambda (v)
                (do
                  (custom-ui-select-section-in-scope scope section)
                  (custom-ui-set-param-in-scope
                    scope
                    p
                    (+ (get p :min) (custom-ui-option-index options v)))))))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

(def ui-lego-micro-base-note-s (section width accent)
  (let ((p (custom-ui-current-base-note-param)))
    (if p
      (subtree :key (str "custom-ui-lego-micro-base-note-" (custom-ui-scope-name))
        (v-stack :width width :height 1.18 :gap 0.16 :align :start
          (label "note" :font-size 7.4 :width width :height 0.52 :color :dim :bg :transparent)
          (number-picker :value (fx-param-value p)
            :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 0
            :step 1
            :noui true :font-size 9.0
            :text-color accent :edit-color :yellow
            :text-align :left
            :width width :height 0.50
            :on-change (custom-ui-param-change-callback p))))
      (label "missing: base_note" :font-size 8 :color :red :bg :transparent))))

(def ui-lego-row (name title decimals unit accent)
  (let ((p (custom-ui-current-param name)))
    (if p
      (instrument-param-mod-wrapper p (str "custom-ui-lego-row-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-lego-row-" (custom-ui-scope-name) "-" name)
          (h-stack :width :fill :height 0.86 :gap 0.35 :align :baseline
            (label title :font-size 8.8 :width 6.2 :color :dim :bg :transparent)
            (number-picker :value (fx-param-value p)
              :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals decimals
              :unit unit
              :noui true :font-size 10.2
              :text-align :left
              :text-color accent :edit-color :yellow
              :width 6.0 :height 0.78
              :on-change (custom-ui-param-change-callback p)))))
      (label (str "missing: " name) :font-size 9 :color :red :bg :transparent))))

(def ui-lego-base-note (width accent)
  (let ((p (custom-ui-current-base-note-param)))
    (if p
      (subtree :key (str "custom-ui-lego-base-note-" (custom-ui-scope-name))
        (v-stack :width width :height 1.12 :gap 0.08 :align :start
          (label "note" :font-size 8.2 :width width :color :dim :bg :transparent)
          (number-picker :value (fx-param-value p)
            :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 0
            :step 1
            :noui true :font-size 10.2
            :text-color accent :edit-color :yellow
            :text-align :left
            :width width :height 0.68
            :on-change (custom-ui-param-change-callback p))))
      (label "missing: base_note" :font-size 9 :color :red :bg :transparent))))

(def ui-adsr-number (name title decimals unit)
  (let ((p (custom-ui-current-param name)))
    (if p
      (instrument-param-mod-wrapper p (str "custom-ui-adsr-number-mod-" (custom-ui-scope-name) "-" name)
        (subtree :key (str "custom-ui-adsr-number-" (custom-ui-scope-name) "-" name)
          (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
            (label title :font-size 10 :color :dim :bg :transparent)
            (number-picker :value (fx-param-value p)
              :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals decimals
              :unit unit
              :noui true :font-size 10.5
              :text-align :center
              :text-color :widget_focus_bg :edit-color :yellow
              :width 5.0 :height 0.95
              :on-change (custom-ui-param-change-callback p)))))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def ui-adsr-number-s (section name title decimals unit)
  (if name
    (let ((p (custom-ui-current-param name)))
      (if p
        (instrument-param-mod-wrapper p (str "custom-ui-adsr-number-mod-" (custom-ui-scope-name) "-" name)
          (subtree :key (str "custom-ui-adsr-number-" (custom-ui-scope-name) "-" name)
            (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
              (label title :font-size 10 :color :dim :bg :transparent)
              (number-picker :value (fx-param-value p)
                :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals decimals
                :unit unit
                :noui true :font-size 10.5
                :text-align :center
                :text-color :widget_focus_bg :edit-color :yellow
                :width 5.0 :height 0.95
                :on-change (custom-ui-param-change-callback-s section p)))))
        (label (str "missing: " name) :font-size 10 :color :red :bg :transparent)))
    (box :width 5.2 :height 1.75
      (v-stack :width 5.2 :height 1.75 :gap 0.0 :align :center
        (label title :font-size 10 :color :dim :bg :transparent)
        (number-picker :value 0 :min 0 :max 0 :decimals decimals
          :unit unit
          :noui true :font-size 10.5
          :text-align :center
          :text-color :dim :edit-color :dim
          :width 5.0 :height 0.95)))))

(def ui-lego-adsr-s (section title attack decay sustain release)
  (let ((scope (custom-ui-current-scope)))
  (box :width (ui-lego-col-w) :height (ui-lego-full-h)
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 12 :padding 0.15
       :on-click (ui-section-select-callback section)
    (v-stack :width :fill :height :fill :gap 0.10
      (adsr-editor
        :attack (ui-param-bound-value attack 5)
        :decay (ui-param-bound-value decay 120)
        :sustain (ui-param-bound-value sustain 0.7)
        :release (if release (ui-param-bound-value release 120) 0)
        :width 22.0 :height 4.0
        :background-color :instrument-control-bg
        :on-change (lambda (env)
          (do
            (custom-ui-select-section-in-scope scope section)
            (custom-ui-set-param-by-name-in-scope scope attack (get env :attack))
            (custom-ui-set-param-by-name-in-scope scope decay (get env :decay))
            (custom-ui-set-param-by-name-in-scope scope sustain (get env :sustain))
            (if release
              (custom-ui-set-param-by-name-in-scope scope release (get env :release))
              false))))
      (box :width :fill :height 1.75 :padding 0.15
        (h-stack :width :fill :gap 0.20 :align :start
          (ui-adsr-number-s section attack "atk" 0 "ms")
          (ui-adsr-number-s section decay "dec" 0 "ms")
          (ui-adsr-number-s section sustain "sus" 2 false)
          (ui-adsr-number-s section release "rel" 0 "ms")))
      (box :width :fill :height 0.35 :h-align :center :v-align :center
        (label title :font-size 8.5 :color :dim :bg :transparent))
      (box :width :fill :flex 1)))))

(def ui-adsr (title attack decay sustain release)
  (let ((scope (custom-ui-current-scope)))
  (box :width 23.1 :height :fill
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 12 :padding 0.15
    (v-stack :width :fill :height :fill :gap 0.10
      (adsr-editor
        :attack (ui-param-bound-value attack 5)
        :decay (ui-param-bound-value decay 120)
        :sustain (ui-param-bound-value sustain 0.7)
        :release (ui-param-bound-value release 120)
        :width 22.0 :height 4.0
        :background-color :instrument-control-bg
        :on-change (lambda (env)
          (do
            (custom-ui-set-param-by-name-in-scope scope attack (get env :attack))
            (custom-ui-set-param-by-name-in-scope scope decay (get env :decay))
            (custom-ui-set-param-by-name-in-scope scope sustain (get env :sustain))
            (custom-ui-set-param-by-name-in-scope scope release (get env :release)))))
      (box :width :fill :height 1.75 :padding 0.15
        (h-stack :width :fill :gap 0.20 :align :start
          (ui-adsr-number attack "atk" 0 "ms")
          (ui-adsr-number decay "dec" 0 "ms")
          (ui-adsr-number sustain "sus" 2 false)
          (ui-adsr-number release "rel" 0 "ms")))
      (box :width :fill :height 0.35 :h-align :center :v-align :center
        (label title :font-size 8.5 :color :dim :bg :transparent))
      (box :width :fill :flex 1)))))

(def ui-adsr-switch (section-a title-a attack-a decay-a sustain-a release-a
                     section-b title-b attack-b decay-b sustain-b release-b)
  (if (= custom-ui-selected-section section-b)
    (ui-adsr title-b attack-b decay-b sustain-b release-b)
    (ui-adsr title-a attack-a decay-a sustain-a release-a)))

(def ui-detail-adsr-s (section title attack decay sustain release)
  (let ((scope (custom-ui-current-scope)))
    (ui-readout-panel-medium-s section
      (h-stack :width :fill :height :fill :gap 0.24 :align :stretch
        (adsr-editor
          :attack (ui-param-bound-value attack 5)
          :decay (ui-param-bound-value decay 120)
          :sustain (ui-param-bound-value sustain 0.7)
          :release (ui-param-bound-value release 120)
          :width 13.2 :height :fill
          :background-color :instrument-control-bg
          :on-change (lambda (env)
            (do
              (custom-ui-select-section-in-scope scope section)
              (custom-ui-set-param-by-name-in-scope scope attack (get env :attack))
              (custom-ui-set-param-by-name-in-scope scope decay (get env :decay))
              (custom-ui-set-param-by-name-in-scope scope sustain (get env :sustain))
              (custom-ui-set-param-by-name-in-scope scope release (get env :release)))))
        (v-stack :width 8.2 :height :fill :gap 0.10 :align :start
          (ui-lego-badge-dark title 7.7 (ui-accent-blue))
          (h-stack :gap 0.14 :align :start
            (ui-lego-micro-num-s section attack "atk" 3.7 0 "ms" (ui-accent-blue))
            (ui-lego-micro-num-s section decay "dec" 3.7 0 "ms" (ui-accent-blue)))
          (h-stack :gap 0.14 :align :start
            (ui-lego-micro-num-s section sustain "sus" 3.7 2 false (ui-accent-blue))
            (ui-lego-micro-num-s section release "rel" 3.7 0 "ms" (ui-accent-blue))))))))

(def ui-detail-adsr-switch-s (section-a title-a attack-a decay-a sustain-a release-a
                              section-b title-b attack-b decay-b sustain-b release-b)
  (if (= custom-ui-selected-section section-b)
    (ui-detail-adsr-s section-b title-b attack-b decay-b sustain-b release-b)
    (ui-detail-adsr-s section-a title-a attack-a decay-a sustain-a release-a)))

(def ui-adsr-compact-s (section title attack decay sustain release)
  (ui-detail-adsr-s section title attack decay sustain release))

(def ui-adsr-compact-switch-s (section-a title-a attack-a decay-a sustain-a release-a
                               section-b title-b attack-b decay-b sustain-b release-b)
  (ui-detail-adsr-switch-s
    section-a title-a attack-a decay-a sustain-a release-a
    section-b title-b attack-b decay-b sustain-b release-b))

;; ui-rack — auto-arrange a flat list of panels into columns based on mode.
;;   mode          :breathe (2 panels per column) or :compact (4 panels per col)
;;   left-panels   ordered list of panels to place LEFT of the ADSR
;;   adsr-form     a pre-built ADSR widget (ui-adsr / ui-adsr-switch / -c variants)
;;   right-panels  ordered list of panels to place RIGHT of the ADSR
;;
;; The instrument doesn't have to know how many fit per column — just list
;; panels in order, pick :breathe or :compact, and the helper chunks them.
(def ui-rack-col-breathe (col)
  (v-stack :width 31.0 :gap 0.10 col))
(def ui-rack-col-compact (col)
  (v-stack :width 20.0 :gap 0.08 col))
(def ui-rack (mode left-panels adsr-form right-panels)
  (if (= mode :compact)
    (h-stack :width :fill :gap 0.35 :align :stretch
      (map ui-rack-col-compact (chunks left-panels 4))
      adsr-form
      (map ui-rack-col-compact (chunks right-panels 4)))
    (h-stack :width :fill :gap 0.4 :align :stretch
      (map ui-rack-col-breathe (chunks left-panels 2))
      adsr-form
      (map ui-rack-col-breathe (chunks right-panels 2)))))

;; Compact ADSR for use alongside ui-panel-c. Fills the available height —
;; the outer h-stack must use `:align :stretch` so the box stretches to the
;; tallest sibling column. ADSR-editor takes the remaining vertical space
;; via `:flex 1`; controls + caption hold their natural height.
(def ui-adsr-c (title attack decay sustain release)
  (let ((scope (custom-ui-current-scope)))
  (box :width 21.0 :height :fill
       :background-color :instrument-control-bg
       :border-width 1 :corner-radius 10 :padding 0.1
    (v-stack :width :fill :height :fill :gap 0.08
      (adsr-editor
        :attack (ui-param-bound-value attack 5)
        :decay (ui-param-bound-value decay 120)
        :sustain (ui-param-bound-value sustain 0.7)
        :release (ui-param-bound-value release 120)
        :width 20.0 :height 4.0
        :background-color :instrument-control-bg
        :on-change (lambda (env)
          (do
            (custom-ui-set-param-by-name-in-scope scope attack (get env :attack))
            (custom-ui-set-param-by-name-in-scope scope decay (get env :decay))
            (custom-ui-set-param-by-name-in-scope scope sustain (get env :sustain))
            (custom-ui-set-param-by-name-in-scope scope release (get env :release)))))
      (box :width :fill :height 1.45 :padding 0.1
        (h-stack :width :fill :gap 0.15 :align :start
          (ui-adsr-number attack "atk" 0 "ms")
          (ui-adsr-number decay "dec" 0 "ms")
          (ui-adsr-number sustain "sus" 2 false)
          (ui-adsr-number release "rel" 0 "ms")))
      (box :width :fill :height 0.3 :h-align :center :v-align :center
        (label title :font-size 7.5 :color :dim :bg :transparent))
      (box :width :fill :flex 1)))))

(def ui-adsr-switch-c (section-a title-a attack-a decay-a sustain-a release-a
                       section-b title-b attack-b decay-b sustain-b release-b)
  (if (= custom-ui-selected-section section-b)
    (ui-adsr-c title-b attack-b decay-b sustain-b release-b)
    (ui-adsr-c title-a attack-a decay-a sustain-a release-a)))

(def midi-fx-ui-param (fx name)
  (nth (filter |p| (= (get p :name) name) (get fx :params)) 0))

(def midi-fx-ui-param-control (name)
  (let ((p (midi-fx-ui-param midi-fx-ui-current-fx name)))
    (if p
      (fx-param-row p midi-fx-ui-current-fx
        (str "custom-midi-fx-ui-" midi-fx-ui-current-name
             "-slot-" (get midi-fx-ui-current-fx :slot-idx) "-" name))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def audio-fx-ui-param (fx name)
  (nth (filter |p| (= (get p :name) name) (get fx :params)) 0))

(def audio-fx-ui-param-control (name)
  (let ((p (audio-fx-ui-param audio-fx-ui-current-fx name)))
    (if p
      (fx-param-row p audio-fx-ui-current-fx
        (str "custom-audio-fx-ui-" (custom-ui-scope-name) "-" name))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def instrument-synth-panel-body (inst)
  (do
    (set! custom-ui-current-kind "instrument")
    (let ((custom (custom-instrument-synth-ui inst)))
      (let ((body
              (if custom
                (box custom
                  :debug-name "custom-synth-wrapper" :padding 0
                  :h-align :start :v-align :stretch)
                (box (fx-param-grid (get inst :synth) false)
                  :debug-name "fallback-synth-wrapper"))))
        (if instrument-mods-open
          (h-stack :debug-name "instrument-mods-inline-body" :height :fill :gap 0.45 :align :stretch
            (instrument-mod-control-panel inst)
            body)
          body)))))

(def midi-fx-panel-body (fx)
  (let ((custom (custom-midi-fx-ui fx)))
    (if custom
      (box
        (v-stack :gap 0.25 custom)
        :debug-name "custom-midi-fx-wrapper" :padding 0 :h-align :start :v-align :start)
      (box (fx-param-grid (get fx :params) fx)
        :debug-name "fallback-midi-fx-wrapper"))))

(def audio-fx-panel-body (fx params)
  (let ((builtin-ui (builtin-audio-fx-ui fx)))
    (if builtin-ui
      builtin-ui
      (do
        (set! custom-ui-current-kind "audio-fx")
        (let ((custom (custom-audio-fx-ui fx)))
          (if custom
            (box
              (v-stack :gap 0.25 custom)
              :debug-name "custom-audio-fx-wrapper" :padding 0 :h-align :start :v-align :start)
            (fx-param-grid params fx)))))))

(def fx-panel-selected? (fx)
  (if (get fx :midi-fx)
    (= selected-midi-fx-slot (get fx :slot-idx))
    (if (get fx :bus-fx)
      (= selected-bus-fx-slot (get fx :slot-idx))
      (= selected-fx-slot (get fx :slot-idx)))))

(def fx-panel-header-bg (selected)
  (if selected :fx-panel-header-selected-bg :fx-panel-header-bg))

(defstate sampler-view-start 0.0)
(defstate sampler-view-duration 0)
(defstate sampler-cursor-time 0.0)
(defstate sampler-active-marker "none")

(def sampler-reset-view ()
  (set! sampler-view-start 0.0)
  (set! sampler-view-duration 0)
  (set! sampler-cursor-time 0.0)
  (set! sampler-active-marker "none"))

(def sampler-set-start-end (start-seconds end-seconds duration)
  (if (> duration 0)
    (do
      (fx-set-instrument-value (dict :idx 2 :control "param") (* 100 (/ start-seconds duration)))
      (fx-set-instrument-value (dict :idx 3 :control "param") (* 100 (/ end-seconds duration))))))

(def sampler-clamp-start (next-start duration)
  (max 0 (min next-start (max 0 (- duration sampler-view-duration)))))

(def sampler-clamp-duration (next-duration duration)
  (max 0.001 (min next-duration (max 0.001 duration))))

(def handle-sampler-waveform-action (event duration)
  (match event.type
    :set-cursor
    (set! sampler-cursor-time event.time)
    :set-selection
    (sampler-set-start-end event.start event.end duration)
    :begin-marker-drag
    (set! sampler-active-marker (if (= event.marker :start) "start" "end"))
    :end-marker-drag
    (set! sampler-active-marker "none")
    :clear-selection
    (sampler-set-start-end 0 duration duration)
    :scroll-view
    (set! sampler-view-start (sampler-clamp-start (+ sampler-view-start event.delta-time) duration))
    :zoom-view
    (let ((cur-duration (if (= sampler-view-duration 0) duration sampler-view-duration)))
      (let ((anchor-ratio (/ (- event.anchor-time sampler-view-start) cur-duration))
            (next-duration (sampler-clamp-duration (/ cur-duration event.factor) duration)))
        (set! sampler-view-duration next-duration)
        (set! sampler-view-start (sampler-clamp-start (- event.anchor-time (* anchor-ratio next-duration)) duration))))
    _
    nil))

(def sampler-panel-drop-sample (event)
  (let ((payload (get event :payload))
      (target (get event :target)))
    (let ((path (get payload :path))
        (track (get target :track)))
      (if path
        (host-command "load-sample-into-track" (dict :track track :path path))
        (status "Drop a sample file, not a folder")))))

(def sampler-param-knob (p key)
  (instrument-param-mod-wrapper p (str key "-mod-wrapper")
    (subtree :key (str key (instrument-param-control-key-mode p))
      (knob-number :label (substring (get p :name) 0 12)
        :value (fx-param-value p)
        :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 1
        :base-value (instrument-param-base-value-prop p)
        :base-min (instrument-param-base-min-prop p) :base-max (instrument-param-base-max-prop p)
        :mod-range-0-slot (instrument-param-knob-mod-slot-prop p 0) :mod-range-0-depth (instrument-param-knob-mod-depth-prop p 0)
        :mod-range-1-slot (instrument-param-knob-mod-slot-prop p 1) :mod-range-1-depth (instrument-param-knob-mod-depth-prop p 1)
        :mod-range-2-slot (instrument-param-knob-mod-slot-prop p 2) :mod-range-2-depth (instrument-param-knob-mod-depth-prop p 2)
        :mod-range-3-slot (instrument-param-knob-mod-slot-prop p 3) :mod-range-3-depth (instrument-param-knob-mod-depth-prop p 3)
        :mod-range-4-slot (instrument-param-knob-mod-slot-prop p 4) :mod-range-4-depth (instrument-param-knob-mod-depth-prop p 4)
        :mod-range-5-slot (instrument-param-knob-mod-slot-prop p 5) :mod-range-5-depth (instrument-param-knob-mod-depth-prop p 5)
        :mod-range-6-slot (instrument-param-knob-mod-slot-prop p 6) :mod-range-6-depth (instrument-param-knob-mod-depth-prop p 6)
        :mod-range-7-slot (instrument-param-knob-mod-slot-prop p 7) :mod-range-7-depth (instrument-param-knob-mod-depth-prop p 7)
        :mod-range-8-slot (instrument-param-knob-mod-slot-prop p 8) :mod-range-8-depth (instrument-param-knob-mod-depth-prop p 8)
        :mod-range-9-slot (instrument-param-knob-mod-slot-prop p 9) :mod-range-9-depth (instrument-param-knob-mod-depth-prop p 9)
        :selected-mod-slot (instrument-selected-mod-slot-prop p)
        :font-size 10.5 :label-font-size 10
        :text-color :dim :label-color :dim
        :width 4.0 :height 2.05
        :on-change (lambda (v) (instrument-set-param-control-value p v))))))

(def sampler-param-button (p key)
  (subtree :key key
    (v-stack :align :center :gap 0.2
      (label (substring (get p :name) 0 12) :font-size 10 :color :dim :bg :transparent)
      (button (if (fx-param-on? p) "ON" "OFF")
        :width 3.2 :height 1.0 :padding 0 :font-size 10
        :background-color (if (fx-param-on? p) (rgba 0.95 0.48 0.18 1.0) :mixer-control-bg)
        :color (if (fx-param-on? p) :black :dim)
        :on-click |x y r| (fx-set-instrument-value p (if (fx-param-on? p) 0 1))))))

(def sampler-param-dropdown (p key)
  (subtree :key key
    (v-stack :align :center :gap 0.2
      (label (substring (get p :name) 0 12) :font-size 10 :color :dim :bg :transparent)
      (dropdown :value (get p :text-value)
        :options (get p :options)
        :on-change (lambda (v) (fx-set-instrument-option p v))
        :width 5.8 :height 1.0 :font-size 9))))

(def sampler-gate-button ()
  (v-stack :align :center :gap 0.2
    (label "gate" :font-size 10 :color :dim :bg :transparent)
    (button (if SEQ.tp-gate "ON" "OFF")
      :width 3.2 :height 1.0 :padding 0 :font-size 10
      :background-color (if SEQ.tp-gate (rgba 0.95 0.48 0.18 1.0) :mixer-control-bg)
      :color (if SEQ.tp-gate :black :dim)
      :on-click |x y r| (do (cool-off-follow) (seq-set-track-param :gate (if SEQ.tp-gate 0 1))))))

(def sampler-param-control (p)
  (let ((key (if (get p :idx)
               (str "sampler-param-" (get p :idx))
               (str "sampler-param-" (get p :name)))))
    (if (get p :boolean)
      (sampler-param-button p key)
      (if (get p :options)
        (sampler-param-dropdown p key)
        (sampler-param-knob p key)))))

(def sampler-param-by-name (params name)
  (nth (filter |p| (= (get p :name) name) params) 0))

(def sampler-main-params (params)
  (filter |p|
    (let ((name (get p :name)))
      (and (not (= name "enabled"))
           (not (= name "warp"))
           (not (= name "mode"))
           (not (= name "bpm"))))
    params))

(def sampler-bpm-control (p)
  (h-stack :gap 0.65 :align :end
    (instrument-param-mod-wrapper p "sampler-param-bpm-mod-wrapper"
      (subtree :key (str "sampler-param-bpm" (instrument-param-control-key-mode p))
        (knob-number :label "bpm"
          :value (fx-param-value p)
          :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 1
          :base-value (instrument-param-base-value-prop p)
          :base-min (instrument-param-base-min-prop p) :base-max (instrument-param-base-max-prop p)
          :mod-range-0-slot (instrument-param-knob-mod-slot-prop p 0) :mod-range-0-depth (instrument-param-knob-mod-depth-prop p 0)
          :mod-range-1-slot (instrument-param-knob-mod-slot-prop p 1) :mod-range-1-depth (instrument-param-knob-mod-depth-prop p 1)
          :mod-range-2-slot (instrument-param-knob-mod-slot-prop p 2) :mod-range-2-depth (instrument-param-knob-mod-depth-prop p 2)
          :mod-range-3-slot (instrument-param-knob-mod-slot-prop p 3) :mod-range-3-depth (instrument-param-knob-mod-depth-prop p 3)
          :mod-range-4-slot (instrument-param-knob-mod-slot-prop p 4) :mod-range-4-depth (instrument-param-knob-mod-depth-prop p 4)
          :mod-range-5-slot (instrument-param-knob-mod-slot-prop p 5) :mod-range-5-depth (instrument-param-knob-mod-depth-prop p 5)
          :mod-range-6-slot (instrument-param-knob-mod-slot-prop p 6) :mod-range-6-depth (instrument-param-knob-mod-depth-prop p 6)
          :mod-range-7-slot (instrument-param-knob-mod-slot-prop p 7) :mod-range-7-depth (instrument-param-knob-mod-depth-prop p 7)
          :mod-range-8-slot (instrument-param-knob-mod-slot-prop p 8) :mod-range-8-depth (instrument-param-knob-mod-depth-prop p 8)
          :mod-range-9-slot (instrument-param-knob-mod-slot-prop p 9) :mod-range-9-depth (instrument-param-knob-mod-depth-prop p 9)
          :selected-mod-slot (instrument-selected-mod-slot-prop p)
          :font-size 10.5 :label-font-size 10
          :text-color :dim :label-color :dim
          :width 4.75 :height 2.05
          :on-change (lambda (v) (instrument-set-param-control-value p v)))))
    (v-stack :gap 0.12 :align :center
      (box :height 0.82)
      (h-stack :gap 0.2
        (button "1/2"
          :width 1.85 :height 0.82 :padding 0 :font-size 8
          :background-color :mixer-control-bg :color :dim
          :on-click |x y r| (fx-set-instrument-value p (min 400 (* (get p :value) 2))))
        (button "2x"
          :width 1.85 :height 0.82 :padding 0 :font-size 8
          :background-color :mixer-control-bg :color :dim
          :on-click |x y r| (fx-set-instrument-value p (max 20 (/ (get p :value) 2))))))))

(def sampler-param-knobs (params inst)
  (h-stack :gap 0.65 :padding 0.55 :align :center
    (sampler-gate-button)
    (each (sampler-main-params params) |p pi|
      (sampler-param-control p))
    (box :width 1.4 :height 1)
    (sampler-param-control (sampler-param-by-name params "warp"))
    (sampler-bpm-control (sampler-param-by-name params "bpm"))))

(def sampler-panel (inst)
  (box :background "fx-panel-bg" :color :instrument-panel-bg :header :fx-panel-header-bg :selected-header :fx-panel-header-selected-bg :selected 0 :padding 0
    :height fx-fixed-panel-height
    :debug-name "sampler-panel"
    :drop-types (list "sample")
    :drop-meta (dict :kind "sampler-panel" :track (get inst :track))
    :drop-hover-border-color :mixer-strip-selected-border
    :on-drop (lambda (event) (sampler-panel-drop-sample event))
    (v-stack :gap 0
      (box :height 0.75 :padding 0 :v-align :center :h-align :start
        (h-stack :gap 0.5 :align :center
          (fx-panel-header-leading-spacer)
          (fx-enabled-toggle (enabled-param (get inst :synth)) false "sampler-enabled")
          (label "Sampler" :font-size 11 :color :white :bg :transparent)
          (instrument-synth-button)
          (instrument-mods-toggle-button)))
      (fx-panel-body "sampler-panel-content"
        (let ((body
                (v-stack
                  (box :background-color :instrument-control-bg :corner-radius 10
                    (v-stack :gap 0.01 :padding 0.15
                      (box :height 0.1)
                      (if (get inst :buffer)
                        (subtree :key (str "sampler-waveform-" (get inst :buffer))
                          (box :width 73 :height 4.85
                            (waveform
                              :height 4.85
                              :header-height 0.3
                              :ruler-font-size 8
                              :ruler-color :dim
                              :ruler-bg :black
                              :grid-major-color :black
                              :grid-minor-color :black
                              :bg :instrument-control-bg
                              :focusable true
                              :marker-selection true
                              :active-marker sampler-active-marker
                              :marker-color :dim
                              :active-marker-color :widget-knob-filled
                              :waveform-color :yellow
                              :inactive-waveform-color '(rgba 0.25 0.25 0.25 1)
                              :buffer (get inst :buffer)
                              :view-start sampler-view-start
                              :view-duration (if (= sampler-view-duration 0) (get inst :duration) sampler-view-duration)
                              :cursor-time sampler-cursor-time
                              :playhead-time (bind-seq "sampler-playhead")
                              :selection-start (bind-seq (get inst :start-time-field))
                              :selection-end (bind-seq (get inst :end-time-field))
                              :time-ruler (dict :mode :seconds)
                              :on-action |event| (handle-sampler-waveform-action event (get inst :duration)))))
                        (box :width 70 :height 4.85 :h-align :center :v-align :center
                          (label "No sample" :font-size 12 :color :dim :bg :transparent)))
                      (sampler-param-knobs (get inst :synth) inst))))))
          (if instrument-mods-open
            (h-stack :debug-name "sampler-mods-inline-body" :height :fill :gap 0.45 :align :stretch
              (instrument-mod-control-panel inst)
              body)
            body))))))

(def modulator-param (inst name)
  (nth (filter |p| (= (get p :name) name) (get inst :synth)) 0))

(def modulator-knob (p label-text key)
  (subtree :key key
    (knob-number :label label-text
      :value (fx-param-value p)
      :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 0
      :font-size 12 :label-font-size 11
      :text-color :dim :label-color :dim
      :width 7.0 :height 4.15 :knob-size 2.55
      :value-align :center
      :on-change (lambda (v) (instrument-set-param-control-value p v)))))

(def modulator-panel (inst)
  (let ((rise-p (modulator-param inst "rise"))
        (fall-p (modulator-param inst "fall")))
    (box :background "fx-panel-bg" :color :instrument-panel-bg :header :fx-panel-header-bg :selected-header :fx-panel-header-selected-bg :selected 0 :padding 0
      :height fx-fixed-panel-height
      :debug-name "modulator-panel"
      (v-stack :gap 0
        (box :height 0.75 :padding 0 :v-align :center :h-align :start
          (h-stack :gap 0.5 :align :center
            (fx-panel-header-leading-spacer)
            (fx-enabled-toggle (enabled-param (get inst :synth)) false "modulator-enabled")
            (label "Modulator" :font-size 11 :color :white :bg :transparent)))
        (fx-panel-body "modulator-panel-content"
          (box :width :fill :height 7.85 :padding 0.35
            :debug-name "modulator-panel-body"
            (h-stack :width :fill :height :fill :gap 1.05 :align :center
              (if rise-p
                (modulator-knob rise-p "rise ms" "modulator-rise-knob")
                (label "missing: rise" :font-size 10 :color :red :bg :transparent))
              (if fall-p
                (modulator-knob fall-p "fall ms" "modulator-fall-knob")
                (label "missing: fall" :font-size 10 :color :red :bg :transparent))
              (box :width 0.45 :height 1)
              (box :width 12.8 :height 5.9 :padding 0.22
                :background-color :black
                :corner-radius 8
                :debug-name "modulator-curve-wrapper"
                (modulator-curve
                  :width 12.25 :height 5.45
                  :rise (if rise-p (fx-param-value rise-p) 0)
                  :fall (if fall-p (fx-param-value fall-p) 0)
                  :phase (bind-seq (get inst :phase-field))
                  :level (bind-seq (get inst :level-field))
                  :max-ms (if rise-p (get rise-p :max) 5000)
                  :background-color :instrument-control-bg
                  :grid-color :dim
                  :curve-color (ui-accent-orange)
                  :fill-color (rgba 1.0 0.48 0.18 0.16))))))))))

(def instrument-panel (inst)
  (if (= (get inst :type) "sampler")
    (sampler-panel inst)
    (if (= (get inst :type) "modulator")
      (modulator-panel inst)
    (box
      (v-stack :debug-name "instrument-panel-vstack" :gap 0
        (box :debug-name "instrument-header-box" :height 0.75 :padding 0 :v-align :center :h-align :start
          (h-stack :debug-name "instrument-header-row" :gap 0.6 :align :center
            (fx-panel-header-leading-spacer)
            (fx-enabled-toggle (enabled-param (get inst :synth)) false "instrument-enabled")
              (h-stack :v-align :center :height 0.75 :gap 2 :padding 0.1
                (label (substring (get inst :display-name) 0 12)
                  :font-size 11  :color :white :bg :transparent)
                  (instrument-synth-button)
                  (instrument-mods-toggle-button))
            
            (box :debug-name "instrument-edit-button" :bg :dark-gray :width 1.2 :height 0.9 :align :center
              :on-click |x y r|
              (host-command "enter-edit-instrument"
                (dict :name SEQ.sidebar-instrument-name))
              (label "edit" :font-size 11 :color :dim :bg :transparent))
            (box :debug-name "instrument-preset-button" :padding 0.3 :width 4 :align :center
              (v-stack
                (box :width 1 :height 0.1)
                (fx-mini-save-icon
                  :on-click |x y r| (sbrowser-enter-preset-save)
                  :active 0)))))
        (fx-panel-body "instrument-content-box"
          (instrument-synth-panel-body inst)))
      :debug-name "instrument-panel"
      :background "fx-panel-bg"
      :color :instrument-panel-bg
      :header :fx-panel-header-bg
      :selected-header :fx-panel-header-selected-bg
      :padding 0
      :height fx-fixed-panel-height
      :selected 0))))

(defwidget black
  :width 2 :height 2
  :shader
  (rgba 0.0 0.0 0 1))

(def fx-empty-track-fallback ()
  (box :width :fill :height :fill :padding 1 :h-align :center :v-align :center
    (v-stack :gap 0.4 :align :center
      (label "Instrument and effects appear here"
        :font-size 12 :color :dim :bg :transparent)
      (compile-progress
        :active (if SEQ.compiling 1 0)
        :width 12 :height 0.3))))

(def selected-bus-effects ()
  (if (fx-has-selected-bus?)
    (nth SEQ.bus-effects selected-bus)
    '()))

(def fx-drop-placeholder-panel ()
  (box :debug-name "fx-drop-placeholder-panel"
       :background-color :buffer-bg
       :corner-radius 10
       :border-color :mixer-strip-border
       :border-width 2
       :drop-types (list "audio-effect" "midi-effect" "effect-instance")
       :drop-meta (dict :kind "fx-append" :chain "append" :track SEQ.current-track :slot -1)
       :drop-hover-border-color :mixer-strip-selected-border
       :drop-hover-background-color :mixer-control-bg
       :on-drop (lambda (event) (fx-drop-on-effect event))
       :height fx-fixed-panel-height
       :width 34
       :padding 0
       :h-align :center
       :v-align :center
    (label "Drop Audio or Midi Effect Here"
      :width 30
      :font-size 12
      :h-align :center
      :color :dim
      :bg :transparent)))

(def fx-bus-selection-panel ()
  (v-stack :padding 0.5 :gap 1
    (h-stack :gap 1
      (each (filter |fx| (> (len (get fx :params)) 0) (selected-bus-effects)) |fx slot-idx|
        (subtree :key (str "bus-fx-panel-" (get fx :bus-idx) "-" (get fx :slot-idx) "-" (get fx :name))
          (fx-panel (get fx :name) (get fx :params) fx)))
      (fx-drop-placeholder-panel))))

(effect-buffer "*track*"
  (if (= SEQ.num-tracks 0)
    (fx-empty-track-fallback)
    (box :padding 0.6
      (v-stack :gap 0.6
        (fx-track-parameters-panel)
        (fx-track-accumulator-panel)
        (fx-track-plocks-panel)))))

(effect-buffer "*fx*"
  (if (fx-has-selected-bus?)
    (fx-bus-selection-panel)
    (if (= SEQ.num-tracks 0)
    (fx-empty-track-fallback)
    (v-stack :padding 0.05 :gap 1 
      (h-stack :gap 1
        (each SEQ.instrument-panel |inst inst-idx|
          (instrument-panel inst))
        (each (filter |fx| (> (len (get fx :params)) 0) SEQ.midi-effects) |fx slot-idx|
          (midi-fx-panel (get fx :name) (get fx :params) fx))
        (each (filter |fx| (> (len (get fx :params)) 0) SEQ.effects) |fx slot-idx|
          (subtree :key (str "audio-fx-panel-" (get fx :slot-idx) "-" (get fx :name))
            (fx-panel (get fx :name) (get fx :params) fx)))
        (fx-drop-placeholder-panel))))))

(define-mode "seq-fx-mode" :read-only true)
(mode-bind-key "seq-fx-mode" "BS" "fx-delete-selected-effect")
(mode-bind-key "seq-fx-mode" "Delete" "fx-delete-selected-effect")
(set-buffer-mode-for "*fx*" "seq-fx-mode")
