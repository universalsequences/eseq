;; Filter Table built-in FX panel.
(module eseq.effects.builtin.filter-table)

(import eseq.effects.builtin.filter-core :refer (eseq.effects.builtin.filter-core/builtin-fx-param))
(import eseq.effects.param-controls :as pc)

;; The generic dynamics knobs only edit base values. Filter Table parameters
;; need the complete modulation contract: in the mods tab the same knob edits
;; the selected source's depth, draws all assigned modulation ranges, and is
;; wrapped by the blue modulation target affordance.
(def %knob (fx label-text p decimals value-scale taper)
  (pc/param-mod-wrapper fx p (str "filter-table-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "filter-table-param-" (get p :idx) (pc/param-control-key-mode fx p))
      (knob-number :label label-text
        :taper taper
        :value (pc/fx-param-value-for fx p)
        :min (pc/param-control-min fx p) :max (pc/param-control-max fx p)
        :value-scale value-scale :decimals decimals
        :base-value (pc/param-base-value-prop fx p)
        :base-min (pc/param-base-min-prop fx p) :base-max (pc/param-base-max-prop fx p)
        :mod-range-0-slot (pc/param-knob-mod-slot-prop fx p 0) :mod-range-0-depth (pc/param-knob-mod-depth-prop fx p 0)
        :mod-range-1-slot (pc/param-knob-mod-slot-prop fx p 1) :mod-range-1-depth (pc/param-knob-mod-depth-prop fx p 1)
        :mod-range-2-slot (pc/param-knob-mod-slot-prop fx p 2) :mod-range-2-depth (pc/param-knob-mod-depth-prop fx p 2)
        :mod-range-3-slot (pc/param-knob-mod-slot-prop fx p 3) :mod-range-3-depth (pc/param-knob-mod-depth-prop fx p 3)
        :selected-mod-slot (pc/param-selected-mod-slot-prop fx p)
        :font-size 9.5 :label-font-size 9.5
        :text-color (pc/param-plock-text-color fx p) :label-color :dim
        :plock-active (if (pc/param-plock-active? fx p) 1 0)
        :plock-default (pc/param-plock-default fx p)
        :plock-color-r (pc/param-plock-color-r)
        :plock-color-g (pc/param-plock-color-g)
        :plock-color-b (pc/param-plock-color-b)
        :width 6.8 :height 2.5 :knob-size 1.85
        :track-color '(rgba 0.4, 0.4, 0.4, 1)
        :on-change (lambda (v) (pc/param-set-control-value fx p v))))))

(def %percent-knob (fx label-text p)
  (%knob fx label-text p 0 100 "linear"))

(def %number-knob (fx label-text p decimals)
  (%knob fx label-text p decimals 1 "linear"))

;; Cutoff spans 40–18000 Hz; a linear knob leaves the musical 40–1000 Hz
;; region on ~5% of the travel. The log taper gives every octave equal arc
;; (typed Hz values and the displayed number are unaffected).
(def %freq-knob (fx label-text p)
  (%knob fx label-text p 0 1 "log"))

(def %spectrum-source (fx)
  (if (get fx :rack-fx)
    (dict :kind :rack-effect :index (get fx :track-idx)
          :rack-slot (get fx :rack-slot) :slot (get fx :slot-idx))
    (if (get fx :bus-fx)
      (dict :kind :bus-effect :index (get fx :bus-idx) :slot (get fx :slot-idx))
      (dict :kind :track-effect :index (get fx :track-idx) :slot (get fx :slot-idx)))))

(def %drop-table (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (let ((path (get payload :path)))
      (if path
        (host-command "set-filter-table-source"
          (dict :track (get target :track)
                :slot (get target :slot)
                :bus (get target :bus)
                :path path))
        (status "Drop an audio sample, not a folder")))))

;; ---- Response editor (eseq-dtx.8) ------------------------------------
;; The nondestructive editor document lives host-side; this section only
;; renders session state from (get fx :editor) and sends commands. The
;; parametric node rides the response-curve-editor's draggable band; the
;; drawn band curve is the widget's own approximation — the authoritative
;; response is the magnitude table above, which previews every edit live.

(def %ed-target (fx)
  (dict :track (get fx :track-idx)
        :bus (if (get fx :bus-fx) (get fx :bus-idx) -1)
        :slot (get fx :slot-idx)))

(def %ed-band-type (kind)
  (if (= kind "lowpass") "lowpass"
    (if (= kind "highpass") "highpass"
      (if (= kind "notch") "notch" "bell"))))

(def %ed-band-action (ed event)
  (if (and (= (get event :band-id) 0)
           (or (= (get event :type) :change-band)
               (= (get event :type) :commit-band)))
    (host-command "filter-table-editor-band"
      (dict :phase (if (= (get event :type) :commit-band) "commit" "change")
            :kind (get (get ed :band) :kind)
            :freq (get event :freq)
            :gain (get event :gain)
            :q (get event :q)))))

(def %ed-bands (ed)
  (let ((band (get ed :band))
        ;; Reference marker: a pinned, disabled point at harmonic 24 — the
        ;; bin the cutoff parameter transposes to its own frequency.
        (marker (dict :id -1 :type "bell" :freq 24 :gain 0 :q 8
                      :enabled false :selected false)))
    (if band
      (list marker
            (dict :id 0
                  :type (%ed-band-type (get band :kind))
                  :freq (get band :freq)
                  :gain (get band :gain)
                  :q (get band :q)
                  :enabled true :selected true))
      (list marker))))

(def %ed-op-button (label-text payload w)
  (button label-text
    :width w :height 0.8 :padding 0 :font-size 7.0
    :background-color :mixer-control-bg :color :dim
    :on-click |x y r| (host-command "filter-table-editor-op" payload)))

(def %ed-node-button (label-text kind)
  (button label-text
    :width 3.0 :height 0.8 :padding 0 :font-size 7.0
    :background-color :mixer-control-bg :color :fg
    :on-click |x y r| (host-command "filter-table-editor-add-node" (dict :kind kind))))

(def %editor-section (fx ed table-key)
  (let ((frames (get ed :frames))
        (sel (get ed :selected-frame)))
    (box :width 36.4 :padding 0.2
      :background-color :instrument-control-bg :corner-radius 8
      (v-stack :width :fill :gap 0.1 :align :stretch
        ;; Toolbar: session state + history + save/close.
        (h-stack :width :fill :height 0.78 :gap 0.3 :align :center
          (label "RESPONSE EDITOR" :font-size 7.5 :color :blue :bg :transparent)
          (label (str "frame " (+ sel 1) "/" frames (if (get ed :dirty) " *" ""))
            :font-size 7.5 :color :dim :bg :transparent)
          (button "<" :width 1.2 :height 0.8 :padding 0 :font-size 7.5
            :background-color :mixer-control-bg :color :fg
            :on-click |x y r|
              (host-command "filter-table-editor-frame" (dict :frame (max 0 (- sel 1)))))
          (button ">" :width 1.2 :height 0.8 :padding 0 :font-size 7.5
            :background-color :mixer-control-bg :color :fg
            :on-click |x y r|
              (host-command "filter-table-editor-frame"
                (dict :frame (min (- frames 1) (+ sel 1)))))
          (button "UNDO" :width 2.6 :height 0.8 :padding 0 :font-size 7.0
            :background-color :mixer-control-bg
            :color (if (get ed :can-undo) :fg :dim)
            :on-click |x y r| (host-command "filter-table-editor-undo" (dict)))
          (button "REDO" :width 2.6 :height 0.8 :padding 0 :font-size 7.0
            :background-color :mixer-control-bg
            :color (if (get ed :can-redo) :fg :dim)
            :on-click |x y r| (host-command "filter-table-editor-redo" (dict)))
          (button "SAVE" :width 2.6 :height 0.8 :padding 0 :font-size 7.0
            :background-color :mixer-control-bg :color :blue
            :on-click |x y r| (host-command "filter-table-editor-save" (dict)))
          (button "CLOSE" :width 2.8 :height 0.8 :padding 0 :font-size 7.0
            :background-color :mixer-control-bg :color :dim
            :on-click |x y r| (host-command "filter-table-editor-close" (dict))))
        ;; Parametric node surface: log-frequency (table harmonics; the
        ;; disabled point pins harmonic 24 = cutoff) against dB.
        (response-curve-editor
          :mode :eq
          :bands (%ed-bands ed)
          :freq-min 1 :freq-max 1024
          :gain-min -24 :gain-max 24
          :q-min 0.25 :q-max 16
          :width 35.9 :height 1.85
          :background-color (rgba 0.030 0.040 0.055 1.0)
          :grid-color (rgba 0.30 0.32 0.36 0.5)
          :stroke-color :blue
          :point-color (rgba 1.0 0.62 0.25 1.0)
          :on-action |event| (%ed-band-action ed event))
        ;; Node + op toolbars. (The magnitude viewer above doubles as the
        ;; table overview: while the editor is open its highlight tracks
        ;; the editor's selected frame, not the frame parameter.)
        (h-stack :width :fill :height 0.68 :gap 0.25 :align :center
          (label "NODE" :font-size 7.0 :color :dim :bg :transparent)
          (%ed-node-button "PEAK" "peak")
          (%ed-node-button "NOTCH" "notch")
          (%ed-node-button "LP" "lowpass")
          (%ed-node-button "HP" "highpass")
          (%ed-node-button "TILT" "tilt")
          (label "FRAME" :font-size 7.0 :color :dim :bg :transparent)
          (%ed-op-button "DUP" (dict :kind "duplicate-frame") 2.4)
          (%ed-op-button "INS" (dict :kind "insert-frame") 2.4)
          (%ed-op-button "DEL" (dict :kind "delete-frame") 2.4)
          (%ed-op-button "KEYS" (dict :kind "interpolate") 2.6))
        (h-stack :width :fill :height 0.68 :gap 0.25 :align :center
          (label "TABLE" :font-size 7.0 :color :dim :bg :transparent)
          (%ed-op-button "SM-SPEC" (dict :kind "smooth-spectral") 3.6)
          (%ed-op-button "SM-TIME" (dict :kind "smooth-temporal") 3.6)
          (%ed-op-button "NORM" (dict :kind "normalize") 2.6)
          (%ed-op-button "TILT-" (dict :kind "tilt" :value -3) 2.6)
          (%ed-op-button "TILT+" (dict :kind "tilt" :value 3) 2.6)
          (%ed-op-button "<<" (dict :kind "shift" :value -0.5) 2.0)
          (%ed-op-button ">>" (dict :kind "shift" :value 0.5) 2.0)
          (%ed-op-button "STR-" (dict :kind "stretch" :value 0.8) 2.4)
          (%ed-op-button "STR+" (dict :kind "stretch" :value 1.25) 2.4))))))

(def filter-table-ui (fx)
  (let ((params (get fx :params)))
    (let ((frame-p (eseq.effects.builtin.filter-core/builtin-fx-param params "frame"))
        (cutoff-p (eseq.effects.builtin.filter-core/builtin-fx-param params "cutoff"))
        (res-p (eseq.effects.builtin.filter-core/builtin-fx-param params "resonance"))
        (mix-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mix"))
        (output-p (eseq.effects.builtin.filter-core/builtin-fx-param params "output"))
        (table-name (get fx :table-name))
        (table-mode (get fx :table-mode))
        (table-engine (get fx :table-engine))
        (table-key (get fx :table-data-key)))
      (v-stack :gap 0.025
        (box :width 36.4 :height (if (get fx :editor) 3.3 4.15) :padding 0.25
          :background-color :instrument-control-bg :corner-radius 8
          :drop-types (list "sample")
          :drop-meta (dict :kind "filter-table-source"
            :track SEQ.current-track
            :bus (if (get fx :bus-fx) (get fx :bus-idx) -1)
            :slot (get fx :slot-idx))
          :drop-hover-border-color :blue
          :on-drop (lambda (event) (%drop-table event))
          (v-stack :width :fill :height :fill :gap 0.08 :align :stretch
            (h-stack :width :fill :height 0.85 :gap 0.35 :align :baseline
              ;; The table name doubles as the preset picker: the dropdown
              ;; lists every loadable .fltab (user filter-tables/ + bundled
              ;; factory presets) and loads the selection as a baked asset.
              ;; Rack slots have no load command target, so they keep a label.
              (if (and (get fx :table-options) (not (get fx :rack-fx)))
                (subtree :key (str "filter-table-preset-" (get fx :slot-idx))
                  (dropdown
                    :value (if table-name table-name "Drop a sample / pick a preset")
                    :bg-color :mixer-strip-bg
                    :border-color :mixer-strip-selected-bg
                    :key (str "filter-table-preset-dd-" (get fx :slot-idx))
                    :options (get fx :table-options)
                    :on-change (lambda (v)
                      (host-command "set-filter-table-source"
                        (dict :track (get fx :track-idx)
                          :bus (if (get fx :bus-fx) (get fx :bus-idx) -1)
                          :slot (get fx :slot-idx)
                          :path (str "fltab:" v))))
                    :width 10.5 :height 0.85 :font-size 9))
                (label (if table-name table-name "Drop an audio sample")
                  :font-size 9.0 :color :fg :bg :transparent))
              ;; Analysis mode of the loaded source; click cycles wavetable →
              ;; single cycle → audio → impulse and re-analyzes the sample.
              ;; Rack slots have no re-analysis command target yet (drops
              ;; share the same limitation); show the mode as text there.
              (if (and table-mode (not (get fx :rack-fx)))
                (button table-mode
                  :width 5.6 :height 0.8 :padding 0 :font-size 7.5
                  :background-color :mixer-strip-bg :color :dim
                  :corner-radius 0
                  :border-color :transparent
                  :on-click |x y r|
                  (host-command "set-filter-table-mode"
                    (dict :track (get fx :track-idx)
                      :bus (if (get fx :bus-fx) (get fx :bus-idx) -1)
                      :slot (get fx :slot-idx)
                      :mode "next")))
                (if table-mode
                  (label table-mode :font-size 7.5 :color :dim :bg :transparent)
                  (box :width 0 :height 0)))
              ;; DSP engine: Spectral (STFT, PDC-compensated latency) vs
              ;; Min Phase (causal FIR, zero latency). Rack slots have no
              ;; engine command target, so they keep a label.
              (if (and table-engine (not (get fx :rack-fx)))
                (subtree :key (str "filter-table-engine-dd-" (get fx :slot-idx))
                  (dropdown
                    :value table-engine
                    :options '("Spectral" "Min Phase")
                    :bg-color :mixer-strip-bg
                    :border-color :transparent
                    :on-change (lambda (v)
                      (host-command "set-filter-table-engine"
                        (dict :track (get fx :track-idx)
                          :bus (if (get fx :bus-fx) (get fx :bus-idx) -1)
                          :slot (get fx :slot-idx)
                          :engine (if (= v "Min Phase") "causal" "spectral"))))
                    :width 6.4 :height 0.8 :font-size 7.5))
                (if table-engine
                  (label table-engine :font-size 7.5 :color :dim :bg :transparent)
                  (box :width 0 :height 0)))
              ;; Response editor toggle (track/bus only; rack slots have no
              ;; editor command target, matching the mode limitation above).
              (if (and table-key (not (get fx :rack-fx)) (not (get fx :editor)))
                (button "EDIT"
                  :width 3.0 :height 0.8 :padding 0 :font-size 7.5
                  :border-color :transparent
                  :background-color :accent :color :fg :on-click |x y r|
                  (host-command "filter-table-editor-open" (%ed-target fx)))
                (box :width 0 :height 0)))
            (if table-key
              (wavetable-viewer
                :data-key table-key :domain :magnitude
                :waves-per-set 64 :set 0
                :wave (if (get fx :editor)
                  (get (get fx :editor) :selected-frame-normalized)
                  (pc/instrument-param-base-value frame-p))
                :wave-normalized true
                :wave-color (if (get fx :editor)
                  (rgba 1.0 0.62 0.25 1.0)
                  (rgba 0.35 0.68 1.0 1.0))
                :inactive-color (rgba 0.20 0.43 0.72 0.34)
                :background-color (rgba 0.035 0.045 0.060 1.0)
                :width 35.9 :height (if (get fx :editor) 1.9 2.75))
              (box :width :fill :height (if (get fx :editor) 1.9 2.75)))))
        (if (and table-key (not (get fx :editor)))
          (eq8-editor
            :width 36.4 :height 2.55
            :bands (list) :selected-band -1
            :source (%spectrum-source fx) :tap-point :pre-fx
            :mode :eq :fft-size 8192 :time-slices 128
            :min-db -96 :max-db 0 :smoothing 0.65
            :freq-min 20 :freq-max 20000
            :response-min-db -48 :response-max-db 8
            :response-data-key table-key
            :response-frame (pc/instrument-param-base-value frame-p)
            :response-cutoff (pc/instrument-param-base-value cutoff-p)
            :response-resonance (pc/instrument-param-base-value res-p)
            :background-color (rgba 0.045 0.055 0.070 1.0)
            :curve-color (rgba 0.78 0.84 0.92 0.96)
            :spectrum-color (rgba 0.18 0.38 0.64 0.30)
            :spectrum-peak-color (rgba 0.36 0.62 0.92 0.58))
          (box :width :fill :height 0))
        (if (get fx :editor)
          (%editor-section fx (get fx :editor) table-key)
          (h-stack :gap 0.6 :align :center
            (if frame-p (%percent-knob fx "frame" frame-p) (box :width 0 :height 0))
            (if cutoff-p (%freq-knob fx "cutoff" cutoff-p) (box :width 0 :height 0))
            (if res-p (%percent-knob fx "resonance" res-p) (box :width 0 :height 0))
            (if mix-p (%percent-knob fx "mix" mix-p) (box :width 0 :height 0))
            (if output-p (%number-knob fx "output" output-p 2) (box :width 0 :height 0))))))))
