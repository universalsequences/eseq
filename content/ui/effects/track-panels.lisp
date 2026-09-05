;; Track-level parameter, accumulator, and parameter-lock panels.
(module eseq.effects.track-panels)

(import eseq.effects.state :as st)
(import eseq.effects.param-controls :as pc)

(export selected-plock-row
        plock-row-selected?
        delete-selected-plock-row
        plock-chip-click
        track-plocks-panel
        step-parameters-panel
        track-parameters-panel)

;; Aliases for unconverted lisp callers (effects/panel-frame.lisp,
;; effects/step-buffer.lisp), the production by-name read of
;; fx-plock-row-selected? in src/ui/input.rs:133, and Rust tests that eval
;; the old flat spellings (src/ui/state_values/tests.rs). The buffers.lisp
;; flat edges (fx-track-parameters-panel, fx-delete-selected-plock-row)
;; retired with eseq.effects.buffers, which imports this module.

(def track-bus-send-field (bus)
  (str "tp-bus-" bus "-send"))

(def mute-group-value (label)
  (if (= label "1") 1
    (if (= label "2") 2
      (if (= label "3") 3
        (if (= label "4") 4
          (if (= label "5") 5
            (if (= label "6") 6
              (if (= label "7") 7
                (if (= label "8") 8
                  0)))))))))

(def set-timebase (label)
  (do
    (eseq.seq-core-state/cool-off-follow)
    (if (seq-has-selection?)
      (seq-plock-timebase label)
      (seq-set-timebase label))))

;; Track-level lock presence rides the SEQV p-lock projection
;; (param-controls.lisp) instead of reading SEQ.track-plocks directly, so a
;; selection change only reruns this panel when one of these locks actually
;; changed.
(def track-param-plock-active? (target)
  (= (reactive-get "SEQV" (str "plk-t-" target "-on")) 1))

(def track-param-plock-default (target fallback)
  (if (track-param-plock-active? target)
    (reactive-get "SEQV" (str "plk-t-" target "-def"))
    fallback))

(def track-bus-send-control (send)
  (v-stack :align :center :gap 0.25
    (h-stack :gap 0.25 :align :baseline
      (label (substring (get send :name) 0 8) :font-size 9 :color :dim :bg :transparent)
      (number-picker
        :value (bind-seq (track-bus-send-field (get send :bus-idx)))
        :min 0 :max 1 :decimals 2
        :noui true :font-size 9 :text-color :dim
        :on-change (lambda (v)
          (do
            (eseq.seq-core-state/cool-off-follow)
            (host-command "set-track-bus-send"
              (dict :bus (get send :bus-idx) :amount v))))
        :width 4 :height 1))
    (box :width 8 :height 2
      (hslider :min 0 :max 1
        :value (bind-seq (track-bus-send-field (get send :bus-idx)))
        :material (eseq.materials/slider-material)
        :on-change (lambda (v)
          (do
            (eseq.seq-core-state/cool-off-follow)
            (host-command "set-track-bus-send"
              (dict :bus (get send :bus-idx) :amount v))))))))

(def plock-set-value (p v)
  (do
    (eseq.seq-core-state/cool-off-follow)
    (host-command "set-track-plock-entry"
      (dict :target (get p :target)
            :step-idx (get p :step-idx)
            :rack-slot (get p :rack-slot)
            :slot-idx (get p :slot-idx)
            :param-idx (get p :param-idx)
            :value v))))

(def plock-set-option (p label)
  (do
    (eseq.seq-core-state/cool-off-follow)
    (host-command "set-track-plock-entry-option"
      (dict :target (get p :target)
            :step-idx (get p :step-idx)
            :rack-slot (get p :rack-slot)
            :slot-idx (get p :slot-idx)
            :param-idx (get p :param-idx)
            :label label))))

(def plock-clear (p)
  (host-command "clear-track-plock-entry"
    (dict :target (get p :target)
          :step-idx (get p :step-idx)
          :rack-slot (get p :rack-slot)
          :slot-idx (get p :slot-idx)
          :param-idx (get p :param-idx)
          :target-track (get p :target-track)
          :network-id (get p :network-id)
          :neuron-idx (get p :neuron-idx))))

(defstate selected-plock-row -1)

(def plock-param-col-width 6.45)
(def plock-lock-col-width 6.35)
(def plock-def-col-width 4.25)
(def plock-col-gap 0.22)

(def plock-row-selected? ()
  (and (>= selected-plock-row 0)
       (< selected-plock-row (len SEQ.track-plocks))))

(def selected-plock-row-preview? ()
  (and (plock-row-selected?)
       (get (nth SEQ.track-plocks selected-plock-row) :preview)))

(def delete-selected-plock-row ()
  (if (plock-row-selected?)
    (if (selected-plock-row-preview?)
      (set! selected-plock-row -1)
      (let ((idx selected-plock-row)
            (next-count (- (len SEQ.track-plocks) 1)))
        (do
          (plock-clear (nth SEQ.track-plocks idx))
          (set! selected-plock-row
            (if (<= next-count 0)
              -1
              (min idx (- next-count 1)))))))
    nil))

(def plock-chip-color (chip alpha)
  (let ((c (if (= (get chip :kind) "def")
        THEME.plock_base
        (list (get chip :color-r) (get chip :color-g) (get chip :color-b)))))
    (rgba (nth c 0) (nth c 1) (nth c 2) alpha)))

(def plock-chip-label (chip)
  (if (get chip :display)
    (substring (get chip :display) 0 6)
    (get chip :label)))

(def plock-chip-click (chip)
  (do
    (eseq.seq-core-state/cool-off-follow)
    (set! selected-plock-row -1)
    (if (seq-has-selection?)
      (host-command "stamp-plock-variant"
        (dict :label (get chip :label)
              :step (eseq.seq-core-state/current-step)))
      (host-command "preview-plock-variant"
        (dict :label (get chip :label))))))

(def plock-chip (chip)
  (let ((current (get chip :current))
      (def-chip (= (get chip :kind) "def"))
      (c (plock-chip-color chip 1.0)))
    (box :key (str "track-plock-chip-" (get chip :kind) "-" (get chip :label))
      :height 1.0
      :width 4.00
      :align :baseline
      :padding 0.014
      :background-color (if current
        (plock-chip-color chip 0.11)
        :mixer-strip-bg
        )
      :border-width (if current 0.75 0.35)
      :border-color (if current c :mixer-strip-selected-bg)
      :corner-radius 4
      :on-click |x y r| (plock-chip-click chip)
      (h-stack :gap 0.16 :align :baseline
        (box :width 0.18 :height 0.28
          :corner-radius 2
          :background-color (if def-chip :transparent c)
          :border-width (if def-chip 1 0)
          :border-color c)
        (box :width 0.2)
        (label (plock-chip-label chip)
          :align :center :flex 1
          :font-size 10.0 :color (if current :black :dim) :bg :transparent)
        (box :width 0.2 )
        ))))

(def plock-domain-title (domain)
  (if (= domain "inst")
    "INST"
    (if (= domain "seq")
      "SEQ"
      (if (= domain "fx")
        "FX"
        "NEURAL"))))

(def plock-domain-count (domain)
  (len (filter |p| (= (plock-row-domain p) domain) SEQ.track-plocks)))

(def plock-row-domain (p)
  (if (get p :domain)
    (get p :domain)
    (if (or (= (get p :target) "neural-instrument")
            (= (get p :target) "neural-effect"))
      "neural"
      (if (or (= (get p :target) "instrument")
              (= (get p :target) "rack-slot-param")
              (= (get p :target) "rack-slot-instrument"))
        "inst"
        (if (= (get p :target) "effect")
          "fx"
          "seq")))))

(def plock-row-title (p)
  (if (= (get p :source) "neuron")
    (str (get p :label) " " (get p :name))
    (get p :name)))

(def plock-row-key (idx suffix)
  (str "track-plock-row-" idx "-" suffix))

(def plock-row-value (p)
  (if (get p :value-field)
    (bind-seq (get p :value-field))
    (get p :value)))

(def plock-group-header (domain)
  (box :height 0.95
    (h-stack :gap 0.35 :align :center
      (label (plock-domain-title domain)
        :font-size 8.5 :color :dim :bg :transparent :width 4.5)
      (box :height 0.05 :width :fill :background-color (rgba 1 1 1 0.10)))))

(def plock-row (p idx)
  (subtree :key (str "track-plock-" idx "-" (get p :target) "-" (get p :step-idx) "-"
      (get p :slot-idx) "-" (get p :param-idx))
    (box :width :fill
      :height 1.14
      :align :baseline
      :padding 0.07
      :background-color (if (= selected-plock-row idx)
        (rgba 0.27 0.78 0.86 0.18)
        (if (= (mod idx 2) 0) (rgba 1 1 1 0.025) :transparent))
      :border-width (if (= selected-plock-row idx) 1 0)
      :border-color (rgba 0.27 0.78 0.86 0.55)
      :corner-radius 2
      :on-click |x y r| (set! selected-plock-row idx)
      (h-stack :width :fill :gap plock-col-gap :align :baseline
        (label (substring (plock-row-title p) 0 12)
          :key (plock-row-key idx "param")
          :font-size 9.2 :width plock-param-col-width
          :color (if (= selected-plock-row idx) :white :dim)
          :bg :transparent)
        (if (or (= (get p :source) "neuron") (get p :preview))
          (label (if (get p :text-value) (get p :text-value) (str (get p :value)))
            :key (plock-row-key idx "lock")
            :font-size 9.2 :width plock-lock-col-width
            :h-align :right :color :yellow :bg :transparent)
          (if (get p :options)
            (dropdown :value (get p :text-value)
              :options (get p :options)
              :key (plock-row-key idx "lock")
              :on-change (lambda (v) (plock-set-option p v))
              :width plock-lock-col-width :height 0.98 :font-size 8.4)
            (number-picker :value (plock-row-value p)
              :min (pc/instrument-param-control-min p) :max (pc/instrument-param-control-max p) :decimals 2
              :key (plock-row-key idx "lock")
              :noui true :font-size 9.2 :text-color :yellow :text-align :right
              :on-change (lambda (v) (plock-set-value p v))
              :width plock-lock-col-width :height 1.0)))
        (label (if (get p :default-text) (get p :default-text) (str (get p :default)))
          :key (plock-row-key idx "def")
          :font-size 9.2 :width plock-def-col-width
          :h-align :right :color :dark-gray :bg :transparent)))))

(def plock-group (domain)
  (if (> (plock-domain-count domain) 0)
    (v-stack :gap 0.12
      (plock-group-header domain)
      (each SEQ.track-plocks |p idx|
        (if (= (plock-row-domain p) domain)
          (plock-row p idx)
          (box :height 0))))
    (box :height 0)))

(def track-plocks-panel ()
  (box :debug-name "track-plocks-panel" :padding 0.72
    (v-stack :gap 0.30
      (if (> (len SEQ.track-plock-variants) 0)
        (label "p-locks" :height 1 :bg :transparent :color :dim :font-size 8)
        )
      
      (wrap :key "track-plock-variant-strip"
        :width :fill :gap 0.18 :row-gap 0.04 :align :start
        (each SEQ.track-plock-variants |chip idx|
          (plock-chip chip)))
      (if (> (len SEQ.track-plocks) 0)
        (v-stack :key "track-plock-table" :width :fill :gap 0.1
          (h-stack :key "track-plock-table-header" :width :fill :gap plock-col-gap
            (label "PARAM" :key "track-plock-header-param"
              :font-size 8.2 :width plock-param-col-width :color :dark-gray :bg :transparent)
            (label "LOCK" :key "track-plock-header-lock"
              :font-size 8.2 :width plock-lock-col-width :h-align :right
              :color :dark-gray :bg :transparent)
            (label "DEF" :key "track-plock-header-def"
              :font-size 8.2 :width plock-def-col-width :h-align :right
              :color :dark-gray :bg :transparent))
          (plock-group "inst")
          (plock-group "seq")
          (plock-group "fx")
          (plock-group "neural"))
        ))))

(def step-param-value (mode)
  (let ((values (eseq.seqv-track-params/seqv-current-param-values mode))
        (step (eseq.seq-core-state/current-step)))
    (if (< step (len values))
      (nth values step)
      0)))

(def step-set-param-direct (mode value)
  ;; The stopped-transport edit path: cursor step, or the p-lock path for a
  ;; selection.
  (do
    (eseq.seq-core-state/cool-off-follow)
    (if (seq-has-selection?)
      (seq-set-step-param-plock
        (eseq.seqv-track-params/seqv-param-keyword mode)
        (eseq.seqv-track-params/seqv-step-param-value mode value))
      (seq-set-step-param
        (eseq.seq-core-state/current-step)
        (eseq.seqv-track-params/seqv-param-keyword mode)
        (eseq.seqv-track-params/seqv-step-param-value mode value)))))

(def step-set-param (mode value)
  ;; Playing with record on: the drag arms live PRINT mode — the value lands
  ;; on the trigger steps the playhead passes, not the cursor step (bead
  ;; eseq-jc9), and only while the mouse is held (step-param-release ends
  ;; it). No cool-off-follow in that branch: the performer is watching the
  ;; playhead, so auto-follow must stay alive. The cursor step rides along
  ;; as the fallback target if the gate races off before dispatch.
  (if (and SEQ.playing SEQ.recording)
    (seq-print-step-param
      (eseq.seq-core-state/current-step)
      (eseq.seqv-track-params/seqv-param-keyword mode)
      (eseq.seqv-track-params/seqv-step-param-value mode value))
    (step-set-param-direct mode value)))

(def step-param-release (mode)
  ;; Hold-to-print: mouse-up on a picker ends that param's print
  ;; immediately. A no-op while nothing is latched (plain clicks, stopped
  ;; transport).
  (seq-print-step-param-release
    (eseq.seqv-track-params/seqv-param-keyword mode)))

(def step-duration-print-context? (mode)
  (and (= mode 1) SEQ.playing SEQ.recording))

(def step-param-min (mode)
  (if (step-duration-print-context? mode) 0.125
    (if (= mode 3) -48
      (if (= mode 1) 0
        (eseq.seqv-track-params/seqv-param-min mode)))))

(def step-param-max (mode)
  (if (step-duration-print-context? mode) 2
    (if (= mode 3) 48
      (if (= mode 1) 128
        (eseq.seqv-track-params/seqv-param-max mode)))))

;; Retrig rate (mode 8) drags on a log taper: equal drag distance is equal
;; interval, so the top of the range sweeps pitch evenly instead of crawling
;; through the rhythmic decade (docs/step-retrig-spec.md).
(def step-param-taper (mode)
  (if (= mode 8) "log"
    ;; Retrig count (mode 7) is 0..127 with most musical action at low counts.
    ;; Square keeps that range precise without making the first repeat require
    ;; the excessive travel of the old cube curve; 127/inf remains reachable.
    (if (= mode 7) "square" "linear")))

;; The retrig pickers span 0..127 / 1..1024 from a one-row strip; pin their
;; full-travel drag distance so a flick is not the whole range.
(def step-param-drag-rows (mode)
  (if (or (= mode 7) (= mode 8)) 24 0))

(def step-param-picker (mode key width)
  (box 
    :corner-radius 16 :width 12 :padding 0.2 :background-color :mixer-strip-bg 
    (h-stack :align :center :gap 0.24
      (box :width 0.5)
      (label (eseq.seqv-track-params/seqv-param-name mode) :font-size 10 :color :dim :bg :transparent :v-align :center :flex 1)
      (number-picker
        :key (str "step-param-" key)
        :value (bind-seq (str "fx-step-value-" key))
        :min (step-param-min mode)
        :max (step-param-max mode)
        :taper (step-param-taper mode)
        :drag-rows (step-param-drag-rows mode)
        :decimals (eseq.seqv-track-params/seqv-param-decimals mode)
        :noui true
        :font-size 10
        :text-color :white
        :on-change (lambda (v) (step-set-param mode v))
        :on-release (lambda () (step-param-release mode))
        :width width
        :height 1.15))))

;; The mixer-v2-* names below resolve through eseq.mixer's compat aliases,
;; NOT an import: importing eseq.mixer would evaluate mixer.lisp, whose
;; top-level (effect-buffer "*mixer*") / define-mode registrations must not
;; ride along into every VM that loads the effects family.
(def step-track-badge ()
  (let ((track SEQ.current-track)
      (muted (eseq.mixer/muted? SEQ.current-track)))
    (box
      :key "step-track-badge"
      :width 4.55 :height 1.0
      :padding 0
      :corner-radius 8
      :v-align :center
      :background-color (rgba
        (eseq.mixer/track-color-r track muted)
        (eseq.mixer/track-color-g track muted)
        (eseq.mixer/track-color-b track muted)
        1.0)
      (label (eseq.mixer/track-collapsed-label track)
        :width 4.55
        :font-size 10
        :v-align :center
        :h-align :center
        :color (if muted :dim :black)
        :bg :transparent))))

(def step-parameters-panel ()
  (box :debug-name "step-parameters-panel" :padding 0.5
    (box :padding 0.0
      :background-color :transparent ;:mixer-strip-bg
      :corner-radius 16
      :border-color :transparent ;:mixer-strip-border    
      (v-stack :gap 0.55
        (box :padding 0.25 :background-color :mixer-strip-selected-bg :corner-radius 12 :v-align :center
          (h-stack :gap 0.45 :align :start
            (step-track-badge)
            (h-stack :key "step-selection-summary" :gap 0.15 :align :center
              (number-label :key "step-cursor-label"
                :value (bind-seq "fx-step-cursor-number")
                :prefix "step " :decimals 0 :width 3.3
                :font-size 8 :color :dim :bg :transparent)
              (label "·" :font-size 8 :color :dim :bg :transparent)
              (number-label :key "step-selection-count-label"
                :value (bind-seq "fx-step-selection-count")
                :suffix " selected" :decimals 0 :width 5.0
                :font-size 8 :color :dim :bg :transparent))))
          (v-stack :gap 0.25 
            (h-stack :gap 0.55 :align :center
              (step-param-picker 3 "transpose" 4.2)
              (step-param-picker 0 "velocity" 4.2)
              )
            (h-stack :gap 0.55 :align :center
              (step-param-picker 1 "duration" 4.2)
              (step-param-picker 4 "pan" 4.2)
              )
            (h-stack :gap 0.55 :align :center
              (step-param-picker 7 "retrig" 4.2)
              (step-param-picker 8 "retrig-rate" 4.2)
              ))
        
        
        )
      )))

(def track-accumulator-panel ()
  (h-stack :debug-name "track-accumulator-panel" :padding 0.00
    (box :padding 0.5
      :background-color :mixer-strip-bg
      :corner-radius 16
      :border-color :mixer-strip-border
      (h-stack :gap 0.55 :align :center
        (v-stack :align :center :gap 0.40
          (label "acc fn" :font-size 8 :color :dim :bg :transparent)
          (dropdown :key "track-accumulator-function"
            :value SEQ.tp-accumulator
            :options SEQ.accumulator-options
            :on-change (lambda (v) (do (eseq.seq-core-state/cool-off-follow) (seq-set-accumulator v)))
            :width 7.0 :height 1.25 :font-size 9))
        (v-stack :align :center :gap 0.40
          (label "acc mode" :font-size 8 :color :dim :bg :transparent)
          (dropdown :key "track-accumulator-mode"
            :value SEQ.tp-accum-mode
            :options SEQ.accum-mode-options
            :on-change (lambda (v) (do (eseq.seq-core-state/cool-off-follow) (seq-set-accum-mode v)))
            :width 6.0 :height 1.25 :font-size 9))
        (v-stack :align :center :gap 0.22
          (v-stack :gap 0.5 :align :center
            (label "acc lim" :font-size 8 :color :dim :bg :transparent)
            (number-picker :key "track-accumulator-limit"
              :value SEQ.tp-accum-limit :min 0 :max 127 :decimals 0
              :noui false :font-size 8 :text-color :dim
              :on-change (lambda (v) (do (eseq.seq-core-state/cool-off-follow) (seq-set-accum-limit v)))
              :width 5.2 :height 1.15)))))))

(def track-parameters-panel ()
  (box :debug-name "track-parameters-strip" :padding 0.0
    (v-stack :gap 0.175
      (box :debug-name "track-primary-parameters-panel" :padding 0.5
        :background-color :mixer-strip-bg
        :corner-radius 16
        :border-color :mixer-strip-border
        (h-stack :gap 1.05 :align :center
          (v-stack :gap 0.5 :align :center
            (label "steps" :font-size 8 :color :dim :bg :transparent)
            (number-picker :value SEQ.tp-num-steps :min 1 :max 256 :decimals 0
              :border-color :none
              :noui false :font-size 8 :text-color :white
              :on-change (lambda (v) (do (eseq.seq-core-state/cool-off-follow) (seq-set-track-param :num-steps v)))
              :width 4.2 :height 1.15))
          
          (v-stack :align :center :gap 0.34
            (label "poly" :font-size 8 :color :dim :bg :transparent)
            (button  (if SEQ.tp-poly "ON" "OFF") :width 3.2 :height 1.3
              :background-color (if SEQ.tp-poly :control-on-bg :poly-off-bg)
              :border-color :none
              :font-size 11
              :color (if SEQ.tp-poly :control-on-fg :poly-off-fg)
              ;; Rack tracks: playback polyphony is per-slot (RackSlotSnapshot::max_polyphony),
              ;; never the track-level param below — route there instead, or this control
              ;; silently edits a value playback ignores.
              :on-click |x y r| (do (eseq.seq-core-state/cool-off-follow)
                (if SEQ.tp-is-rack
                  (host-command "set-rack-slot-max-polyphony"
                    (dict :track SEQ.current-track :slot SEQ.tp-rack-slot-idx :value (if SEQ.tp-poly 1 4)))
                  (seq-set-track-param :poly (if SEQ.tp-poly 0 1))))
              )
            )
          (v-stack :gap 0.5 :align :center
            (label "voices" :font-size 8 :color :dim :bg :transparent)
            (number-picker :value SEQ.tp-max-polyphony :min 1 :max 12 :decimals 0
              :border-color :none
              :noui false :font-size 8 :text-color :white
              :on-change (lambda (v) (do (eseq.seq-core-state/cool-off-follow)
                  (if SEQ.tp-is-rack
                    (host-command "set-rack-slot-max-polyphony"
                      (dict :track SEQ.current-track :slot SEQ.tp-rack-slot-idx :value v))
                    (seq-set-track-param :voices v))))
              :width 3.4 :height 1.15)
            )
          (v-stack :align :center :gap 0.40
            (label "scale" :font-size 8 :color :dim :bg :transparent)
            (dropdown :value SEQ.tp-fts
              :options SEQ.fts-options
              :on-change (lambda (v) (do (eseq.seq-core-state/cool-off-follow) (seq-set-fts v)))
              :width 7.0 :height 1.25 :font-size 9))
          
          ))
      (box :debug-name "track-groove-parameters-panel" :padding 0.5
        :background-color :mixer-strip-bg
        :corner-radius 16
        :border-color :mixer-strip-border
        (h-stack :gap 1.05 :align :center
          (v-stack :align :center :gap 0.40
            (label "swg res" :font-size 8 :color :dim :bg :transparent)
            (dropdown :value SEQ.tp-swing-resolution
              :key "track-swing-resolution"
              :options '("1/16" "1/8" "1/4" "1/2")
              :on-change (lambda (v) (do (eseq.seq-core-state/cool-off-follow) (seq-set-swing-resolution v)))
              :plock-active (if (track-param-plock-active? "swing-resolution") 1 0)
              :plock-color-r (pc/param-plock-color-r)
              :plock-color-g (pc/param-plock-color-g)
              :plock-color-b (pc/param-plock-color-b)
              :width 5.0 :height 1.25 :font-size 9))
          (v-stack :align :center :gap 0.22
            (v-stack :gap 0.5 :align :center
              (label "swing" :font-size 8 :color :dim :bg :transparent)
              (number-picker :value SEQ.tp-swing :min 50 :max 75 :decimals 1
                :key "track-swing"
                :border-color :none
                :noui false :font-size 8 :text-color :dim
                :plock-active (if (track-param-plock-active? "swing") 1 0)
                :plock-default (track-param-plock-default "swing" SEQ.tp-swing)
                :plock-color-r (pc/param-plock-color-r)
                :plock-color-g (pc/param-plock-color-g)
                :plock-color-b (pc/param-plock-color-b)
                :on-change (lambda (v) (do (eseq.seq-core-state/cool-off-follow) (seq-set-track-param :swing v)))
                :width 5.2 :height 1.15))
            )
          
          (v-stack :align :center :gap 0.40
            (label "timebase" :font-size 8 :color :dim :bg :transparent)
            (dropdown :value SEQ.tp-timebase
              :key "track-timebase"
              :options st/seq-timebase-options
              :on-change (lambda (v) (set-timebase v))
              :plock-active (if (track-param-plock-active? "timebase") 1 0)
              :plock-color-r (pc/param-plock-color-r)
              :plock-color-g (pc/param-plock-color-g)
              :plock-color-b (pc/param-plock-color-b)
              :width 6.0 :height 1.25 :font-size 9))
          
          (v-stack :align :center :gap 0.40
            (label "mute grp" :font-size 8 :color :dim :bg :transparent)
            (dropdown :value SEQ.tp-mute-group
              :options SEQ.mute-group-options
              :on-change (lambda (v)
                (do
                  (eseq.seq-core-state/cool-off-follow)
                  (seq-set-track-param :mute-group (mute-group-value v))))
              :width 5.4 :height 1.25 :font-size 9))
          )))))
