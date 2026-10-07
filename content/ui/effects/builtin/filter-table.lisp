;; Filter Table built-in FX panel.
(module eseq.effects.builtin.filter-table)

(import eseq.kinds :refer (table-editor table-editor-close! table-editor-band!
                           table-editor-op! table-editor-add-node! table-editor-frame!
                           table-editor-undo! table-editor-redo! table-editor-save!))
(import eseq.effects.builtin.filter-core :refer (builtin-fx-param))
(import eseq.effects.param-controls :as pc)
(import eseq.effects.devices :as dv)

(export filter-table-ui)

;; The generic dynamics knobs only edit base values. Filter Table parameters
;; need the complete modulation contract: in the mods tab the same knob edits
;; the selected source's depth, draws all assigned modulation ranges, and is
;; wrapped by the blue modulation target affordance.
(def parameter-knob (fx label-text p decimals value-scale taper)
  (pc/param-mod-wrapper fx p (str "filter-table-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "filter-table-param-" (get p :idx) (pc/param-control-key-mode fx p))
      (knob-number :label label-text
        :taper taper
        :value (pc/fx-param-value-for fx p)
        :min (pc/param-control-min fx p) :max (pc/param-control-max fx p)
        :value-scale value-scale :decimals decimals
        :base-value (pc/param-base-value-prop fx p)
        :mod-offset (pc/param-mod-offset-for fx p)
        :mod-scale (pc/param-mod-scale-for fx p)
        :unit (pc/param-control-unit fx p)
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
        :width 6.8 :height 2.22 :knob-size 2.75
        :on-change (lambda (v) (pc/param-set-control-value fx p v))))))

(def percent-knob (fx label-text p)
  (parameter-knob fx label-text p 0 (eseq.effects.param-controls/percent-scale fx p) "linear"))

(def number-knob (fx label-text p decimals)
  (parameter-knob fx label-text p decimals 1 "linear"))

;; Cutoff spans 40–18000 Hz; a linear knob leaves the musical 40–1000 Hz
;; region on ~5% of the travel. The log taper gives every octave equal arc
;; (typed Hz values and the displayed number are unaffected).
(def freq-knob (fx label-text p)
  (parameter-knob fx label-text p 0 1 "log"))

(def spectrum-source (fx)
  (if (get fx :rack-fx)
    (dict :kind :rack-effect :index (get fx :track-idx)
          :rack-slot (get fx :rack-slot) :slot (get fx :slot-idx))
    (if (get fx :bus-fx)
      (dict :kind :bus-effect :index (get fx :bus-idx) :slot (get fx :slot-idx))
      (dict :kind :track-effect :index (get fx :track-idx) :slot (get fx :slot-idx)))))

(def command-target (fx)
  (dict :track (get fx :track-idx)
        :rack-slot (get fx :rack-slot)
        :slot (get fx :slot-idx)
        :bus (if (get fx :bus-fx) (get fx :bus-idx) -1)))

(def set-source (fx path)
  (host-command "set-filter-table-source"
    (merge (command-target fx) :path path)))

(def set-engine (fx engine)
  (host-command "set-filter-table-engine"
    (merge (command-target fx) :engine engine)))

(def drop-table (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (let ((path (get payload :path)))
      (if path
        (host-command "set-filter-table-source"
          (merge target :path path))
        (status "Drop an audio sample, not a folder")))))

;; ---- Response editor (eseq-dtx.8) ------------------------------------
;; The nondestructive editor document lives host-side (the table-editor
;; kind); this section only renders the session's state and sends its
;; actions. The parametric node rides the response-curve-editor's draggable
;; band; the drawn band curve is the widget's own approximation — the
;; authoritative response is the magnitude table above, which previews every
;; edit live.

;; Whether the response editor session edits d (fx's device).
(def editing? (d)
  (let ((te table-editor))
    (and d te.open (= te.device d))))

(def ed-band-type (kind)
  (if (= kind "lowpass") "lowpass"
    (if (= kind "highpass") "highpass"
      (if (= kind "notch") "notch" "bell"))))

(def ed-band-action (te event)
  (if (and (= (get event :band-id) 0)
           (or (= (get event :type) :change-band)
               (= (get event :type) :commit-band)))
    (table-editor-band! te.band-kind (get event :freq) (get event :gain) (get event :q)
      :phase (if (= (get event :type) :commit-band) "commit" "change"))))

(def ed-bands (te)
  ;; Reference marker: a pinned, disabled point at harmonic 24 — the bin
  ;; the cutoff parameter transposes to its own frequency.
  (let ((marker (dict :id -1 :type "bell" :freq 24 :gain 0 :q 8
                      :enabled false :selected false)))
    (if te.band-kind
      (list marker
            (dict :id 0
                  :type (ed-band-type te.band-kind)
                  :freq te.band-freq
                  :gain te.band-gain
                  :q te.band-q
                  :enabled true :selected true))
      (list marker))))

;; A frame or table op button: (table-editor-op! kind option value …).
(def ed-op-button (label-text op w)
  (button label-text
    :width w :height 0.8 :padding 0 :font-size 7.0
    :background-color :mixer-control-bg :color :dim
    :on-click |x y r| (apply table-editor-op! op)))

(def ed-node-button (label-text kind)
  (button label-text
    :width 3.0 :height 0.8 :padding 0 :font-size 7.0
    :background-color :mixer-control-bg :color :fg
    :on-click |x y r| (table-editor-add-node! kind)))

(def editor-section (te)
  (let ((frames te.frames)
        (sel te.selected-frame))
    (box :width 36.4 :padding 0.2
      :background-color :instrument-control-bg :corner-radius 8
      (v-stack :width :fill :gap 0.1 :align :stretch
        ;; Toolbar: session state + history + save/close.
        (h-stack :width :fill :height 0.78 :gap 0.3 :align :center
          (label "RESPONSE EDITOR" :font-size 7.5 :color :blue :bg :transparent)
          (label (str "frame " (+ sel 1) "/" frames (if te.dirty " *" ""))
            :font-size 7.5 :color :dim :bg :transparent)
          (button "<" :width 1.2 :height 0.8 :padding 0 :font-size 7.5
            :background-color :mixer-control-bg :color :fg
            :on-click |x y r| (table-editor-frame! (max 0 (- sel 1))))
          (button ">" :width 1.2 :height 0.8 :padding 0 :font-size 7.5
            :background-color :mixer-control-bg :color :fg
            :on-click |x y r| (table-editor-frame! (min (- frames 1) (+ sel 1))))
          (button "UNDO" :width 2.6 :height 0.8 :padding 0 :font-size 7.0
            :background-color :mixer-control-bg
            :color (if te.can-undo :fg :dim)
            :on-click |x y r| (table-editor-undo!))
          (button "REDO" :width 2.6 :height 0.8 :padding 0 :font-size 7.0
            :background-color :mixer-control-bg
            :color (if te.can-redo :fg :dim)
            :on-click |x y r| (table-editor-redo!))
          (button "SAVE" :width 2.6 :height 0.8 :padding 0 :font-size 7.0
            :background-color :mixer-control-bg :color :blue
            :on-click |x y r| (table-editor-save!))
          (button "CLOSE" :width 2.8 :height 0.8 :padding 0 :font-size 7.0
            :background-color :mixer-control-bg :color :dim
            :on-click |x y r| (table-editor-close!)))
        ;; Parametric node surface: log-frequency (table harmonics; the
        ;; disabled point pins harmonic 24 = cutoff) against dB.
        (response-curve-editor
          :mode :eq
          :bands (ed-bands te)
          :freq-min 1 :freq-max 1024
          :gain-min -24 :gain-max 24
          :q-min 0.25 :q-max 16
          :width 35.9 :height 1.85
          :background-color :filter-table-editor-bg
          :grid-color :filter-table-editor-grid
          :stroke-color :blue
          :point-color :filter-table-wave
          :on-action |event| (ed-band-action te event))
        ;; Node + op toolbars. (The magnitude viewer above doubles as the
        ;; table overview: while the editor is open its highlight tracks
        ;; the editor's selected frame, not the frame parameter.)
        (h-stack :width :fill :height 0.68 :gap 0.25 :align :center
          (label "NODE" :font-size 7.0 :color :dim :bg :transparent)
          (ed-node-button "PEAK" "peak")
          (ed-node-button "NOTCH" "notch")
          (ed-node-button "LP" "lowpass")
          (ed-node-button "HP" "highpass")
          (ed-node-button "TILT" "tilt")
          (label "FRAME" :font-size 7.0 :color :dim :bg :transparent)
          (ed-op-button "DUP" (list "duplicate-frame") 2.4)
          (ed-op-button "INS" (list "insert-frame") 2.4)
          (ed-op-button "DEL" (list "delete-frame") 2.4)
          (ed-op-button "KEYS" (list "interpolate") 2.6))
        (h-stack :width :fill :height 0.68 :gap 0.25 :align :center
          (label "TABLE" :font-size 7.0 :color :dim :bg :transparent)
          (ed-op-button "SM-SPEC" (list "smooth-spectral") 3.6)
          (ed-op-button "SM-TIME" (list "smooth-temporal") 3.6)
          (ed-op-button "NORM" (list "normalize") 2.6)
          (ed-op-button "TILT-" (list "tilt" :value -3) 2.6)
          (ed-op-button "TILT+" (list "tilt" :value 3) 2.6)
          (ed-op-button "<<" (list "shift" :value -0.5) 2.0)
          (ed-op-button ">>" (list "shift" :value 0.5) 2.0)
          (ed-op-button "STR-" (list "stretch" :value 0.8) 2.4)
          (ed-op-button "STR+" (list "stretch" :value 1.25) 2.4))))))

;; The Filter Table panel: its device's table and the params' knobs (the
;; plain param grid while the device is not published).
(def filter-table-ui (fx)
  (let ((d (dv/fx-device fx)))
    (if d
      (table-panel fx d)
      (eseq.effects.param-grid/fx-param-grid (get fx :params) fx))))

(def table-panel (fx d)
  (let ((params (get fx :params))
        (editing (editing? d))
        (frame-p (eseq.effects.builtin.filter-core/builtin-fx-param params "frame"))
        (cutoff-p (eseq.effects.builtin.filter-core/builtin-fx-param params "cutoff"))
        (res-p (eseq.effects.builtin.filter-core/builtin-fx-param params "resonance"))
        (mix-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mix"))
        (output-p (eseq.effects.builtin.filter-core/builtin-fx-param params "output"))
        (table-name d.table-name)
        (table-mode d.table-mode)
        (table-engine d.table-engine)
        (table-key d.table-data-key)
        (table-options d.table-options))
    (v-stack :gap 0.025
      (box :width 36.4 :height (if editing 3.3 7.62) :padding 0.35
        :background-color :instrument-control-bg :corner-radius 16
        :drop-types (list "sample")
        :drop-meta (merge (command-target fx) :kind "filter-table-source")
        :drop-hover-border-color :blue
        :on-drop (lambda (event) (drop-table event))
        (v-stack :width :fill :height :fill :gap 0.12 :align :stretch
          (box :height 0.1)
          (h-stack :width :fill :height 0.85 :gap 0.35 :align :baseline
            (box :width 0.5)
            ;; The table name doubles as the preset picker: the dropdown
            ;; lists every loadable .fltab (user filter-tables/ + bundled
            ;; factory presets) and loads the selection as a baked asset.
            (if table-options
              (subtree :key (str "filter-table-preset-" (get fx :slot-idx))
                (dropdown
                  :value (if table-name table-name "Drop a sample / pick a preset")
                  :bg-color :mixer-strip-bg
                  :border-color :buffer-bg
                  :badge-color :filter-table-badge-bg
                  :chevron-color :black
                  :key (str "filter-table-preset-dd-" (get fx :slot-idx))
                  :options table-options
                  :on-change (lambda (v)
                    (set-source fx (str "fltab:" v)))
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
            ;; DSP engine: Spectral (STFT; adds compensated latency to the
            ;; entire project's output) vs Min Phase (causal FIR; adds no
            ;; output latency).
            (if table-engine
              (subtree :key (str "filter-table-engine-dd-" (get fx :slot-idx))
                (dropdown
                  :value table-engine
                  :options '("Spectral" "Min Phase")
                  :bg-color :mixer-strip-bg
                  :border-color :buffer-bg
                  :badge-color :filter-table-badge-bg
                  :chevron-color :black
                  :on-change (lambda (v)
                    (set-engine fx (if (= v "Min Phase") "causal" "spectral")))
                  :width 6.4 :height 0.85 :font-size 9))
              (if table-engine
                (label table-engine :font-size 7.5 :color :dim :bg :transparent)
                (box :width 0 :height 0)))
            ;; Response editor toggle (track/bus only; rack slots have no
            ;; editor command target, matching the mode limitation above).
            ;; note: removed due to author on 8/27/2026 (reason: too complicated)
            ; (if (and table-key (not (get fx :rack-fx)) (not editing))
            ; (button "EDIT"
            ; :width 3.0 :height 0.8 :padding 0 :font-size 7.5
            ; :border-color :transparent
            ; :background-color :accent :color :fg :on-click |x y r|
            ; (table-editor-open! d))
            ;(box :width 0 :height 0))
            )
          (if table-key
            (wavetable-viewer
              :data-key table-key :domain :magnitude
              :waves-per-set 64 :set 0
              :wave (if editing
                #'table-editor.selected-frame-normalized
                (pc/param-effective-ratio frame-p))
              :wave-normalized true
              :wave-color :filter-table-wave
              :inactive-color :filter-table-wave-inactive
              :background-color :filter-table-wave-bg
              :width 35.9 :height (if editing 1.9 2.75))
            (box :width :fill :height (if editing 1.9 2.75)))
          (if (and table-key (not editing))
            (eq8-editor
              :width 35.3 :height 2.05
              :bands (list) :selected-band -1
              :source (spectrum-source fx) :tap-point :pre-fx
              :mode :eq :fft-size 8192 :time-slices 128
              :min-db -96 :max-db 0 :smoothing 0.65
              :freq-min 20 :freq-max 20000
              :response-min-db -48 :response-max-db 8
              :response-data-key table-key
              :response-frame (pc/param-effective-ratio frame-p)
              :response-cutoff (pc/param-effective-value cutoff-p)
              :response-resonance (pc/param-effective-ratio res-p)
              :background-color :mixer-control-bg
              :curve-color :filter-table-response
              :spectrum-color :filter-table-spectrum
              :spectrum-peak-color :filter-table-spectrum-peak)
            (box :width :fill :height 0))))
      (if editing
        (editor-section table-editor)
        (h-stack :gap 0.6 :align :center
          (if frame-p (percent-knob fx "frame" frame-p) (box :width 0 :height 0))
          (if cutoff-p (freq-knob fx "cutoff" cutoff-p) (box :width 0 :height 0))
          (if res-p (percent-knob fx "resonance" res-p) (box :width 0 :height 0))
          (if mix-p (percent-knob fx "mix" mix-p) (box :width 0 :height 0))
          (if output-p (number-knob fx "output" output-p 2) (box :width 0 :height 0)))))))
