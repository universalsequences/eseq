; MD Snare UI — Machinedrum-style snare machines, laid out like
; membrane-snare-mk2: two 2-panel columns (MACHINE/NOISE, FILTER/DIRT) with
; a full-height engine column on the right that swaps with the machine
; selector. The right column carries the three controls that matter most for
; the chosen machine as oversized knobs; for the PI machines that is the
; WIRES section (SNARES level, TUNE, RING) so the snare-wire layer is finally
; tunable rather than a fixed-pitch buzz behind RVOL.
;
; Param names match dsp.lisp. Section titles are plain :fg text; knobs use
; the default cyan accent except the engine column, which takes a colour per
; machine family (TRX orange, EFM blue, PI green).

(def mds-engine-options () '("TRX-SD" "EFM-SD" "PI-SD" "TRX-RS" "EFM-RS" "PI-RS"))

(def mds-engine-index ()
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param "engine")))
    (let ((e (if p (round (if (get p :value-field) (reactive-get "SEQ" (get p :value-field)) (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value p)))) 1)))
      (if (= e 2) 2 (if (= e 3) 3 (if (= e 4) 4 (if (= e 5) 5 (if (= e 6) 6 1))))))))

(def mds-num-w () 4.6)
(def mds-grid-w () (+ (* 2 (mds-num-w)) 0.18))
(def mds-knob-w () 4.55)
(def mds-c () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def mds-trx-c () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def mds-efm-c () (eseq.effects.custom-ui-lego/ui-accent-blue))
(def mds-pi-c () (eseq.effects.custom-ui-lego/ui-accent-green))

(def mds-title-w (title w)
  (box :width w :height 0.82 :v-align :end
    (label title :font-size 9.2 :width w :height 0.82 :color :fg :bg :transparent)))
(def mds-title (title) (mds-title-w title (mds-num-w)))
(def mds-num-w-s (name title unit decimals w color)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title w decimals unit color))
(def mds-num (name title unit decimals)
  (mds-num-w-s name title unit decimals (mds-num-w) (mds-c)))
(def mds-knob (name title decimals color)
  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 name title (mds-knob-w) color decimals))
; same height as a micro-num so a panel with empty grid slots keeps its title
; at the same vertical spot as a full one
(def mds-blank () (box :width (mds-num-w) :height 1.84))

; panel skeleton: title + up to 3 pickers in a 2x2 grid on the left, knobs right
(def mds-panel (title n1 n2 n3 knobs)
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width (mds-grid-w) :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start (mds-title title) n1)
        (h-stack :gap 0.18 :align :start n2 n3))
      (box :width :fill :height 0.1)
      knobs)))
(def mds-knobs-2 (a b) (h-stack :gap 0.10 :align :start a b))
(def mds-knobs-3 (a b c) (h-stack :gap 0.10 :align :start a b c))

; MACHINE: the machine selector sits where the first picker would, so it reads
; as the panel's headline; base note and humanize below it.
(def mds-machine ()
  (mds-panel "MACHINE"
    (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "engine" "MACHINE" (mds-num-w) (mds-engine-options) (mds-c))
          (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 (mds-num-w) (mds-c))
          (mds-num "humanize" "HMNZ" false 2)
    (mds-knobs-3 (mds-knob "ptch" "PTCH" 0 (mds-c))
          (mds-knob "dec" "DEC" 0 (mds-c))
          (mds-knob "level" "LEVEL" 2 (mds-c)))))

; NOISE: the shared snap/noise layer (TRX and EFM use it; PI has its own wires)
(def mds-noise ()
  (mds-panel "NOISE"
    (mds-num "ndec" "NDEC" "ms" 0)
          (mds-num "hpf" "HPF" "Hz" 0)
          (mds-num "tone" "TONE" false 2)
    (mds-knobs-2 (mds-knob "snap" "SNAP" 2 (mds-c))
          (mds-knob "noise" "NOISE" 2 (mds-c)))))

; FILTER: the MD track filter (HP at FLTF, LP at FLTF+FLTW) plus the EQ band
(def mds-filter ()
  (mds-panel "FILTER"
    (mds-num "fltq" "FLTQ" false 2)
          (mds-num "eqf" "EQF" "Hz" 0)
          (mds-num "eqg" "EQG" "dB" 1)
    (mds-knobs-2 (mds-knob "fltf" "FLTF" 0 (mds-c))
          (mds-knob "fltw" "FLTW" 0 (mds-c)))))

; DIRT: sample-rate reduction, distortion, and the AM stage
(def mds-dirt ()
  (mds-panel "DIRT"
    (mds-num "amd" "AMD" false 2)
          (mds-num "amf" "AMF" "Hz" 0)
          (mds-blank)
    (mds-knobs-2 (mds-knob "srr" "SRR" 2 (mds-c))
          (mds-knob "dist" "DIST" 2 (mds-c)))))

; engine column: one full-height column spanning both panel rows, with three
; oversized knobs on a wide pitch so the labels never shrink. Which three
; depends on the machine.
(def mds-engine-w () 18.0)
(def mds-engine-h ()
  (+ (* 2 (eseq.effects.custom-ui-lego/ui-lego-dense-h)) (eseq.effects.custom-ui-lego/ui-lego-gap)))
(def mds-big-knob (name title decimals color)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 name title 5.6 3.12 3.12 color decimals))
(def mds-eng-num (name title unit decimals color)
  (mds-num-w-s name title unit decimals 5.6 color))
(def mds-eng-blank () (box :width 5.6 :height 1.84))
(def mds-engine-column (title color top k1 k2 k3 b1 b2 b3)
  (eseq.effects.custom-ui-lego/ui-lego-panel-width-s (mds-engine-w) (mds-engine-h) 0 :instrument-group-bg
    (box :width :fill :height :fill :v-align :start
      (v-stack :width :fill :gap 0.30 :align :start
        (h-stack :gap 0.22 :align :start (mds-title title) top)
        (h-stack :gap 0.10 :align :start k1 k2 k3)
        (h-stack :gap 0.22 :align :end b1 b2 b3)))))

; TRX-SD: two-partial analog body; BUMP is the pitch-envelope depth, BENV its
; time, TUNE spreads the second partial.
(def mds-trx-sd ()
  (mds-engine-column "TRX-SD" (mds-trx-c)
    (mds-eng-num "benv" "BENV" "ms" 0 (mds-trx-c))
    (mds-big-knob "bump" "BUMP" 0 (mds-trx-c))
    (mds-big-knob "tone" "TONE" 2 (mds-trx-c))
    (mds-big-knob "clip" "CLIP" 2 (mds-trx-c))
    (mds-eng-num "tune" "TUNE" false 2 (mds-trx-c))
    (mds-eng-num "snap" "SNAP" false 2 (mds-trx-c))
    (mds-eng-blank)))

; EFM-SD: FM body; MOD is the index, MFRQ/MDEC the modulator pitch and decay
(def mds-efm-sd ()
  (mds-engine-column "EFM-SD" (mds-efm-c)
    (mds-eng-num "mdec" "MDEC" "ms" 0 (mds-efm-c))
    (mds-big-knob "mod_amt" "MOD" 2 (mds-efm-c))
    (mds-big-knob "mfrq" "MFRQ" 0 (mds-efm-c))
    (mds-big-knob "noise" "NOISE" 2 (mds-efm-c))
    (mds-eng-num "ndec" "NDEC" "ms" 0 (mds-efm-c))
    (mds-eng-num "hpf" "HPF" "Hz" 0 (mds-efm-c))
    (mds-eng-num "clip" "CLIP" false 2 (mds-efm-c))))

; PI wires: SNARES is the wire level, TUNE its bandpass/ring pitch, RING the
; shell overtone mix. HARD/TENS shape the head excitation and live below.
(def mds-wires (title b1 b2 b3)
  (mds-engine-column title (mds-pi-c)
    (mds-eng-num "rdec" "RDEC" "ms" 0 (mds-pi-c))
    (mds-big-knob "rvol" "SNARES" 2 (mds-pi-c))
    (mds-big-knob "rtun" "TUNE" 0 (mds-pi-c))
    (mds-big-knob "ring" "RING" 2 (mds-pi-c))
    b1 b2 b3))
(def mds-pi-sd ()
  (mds-wires "PI-SD WIRES"
    (mds-eng-num "hard" "HARD" false 2 (mds-pi-c))
    (mds-eng-num "tens" "TENS" false 2 (mds-pi-c))
    (mds-eng-num "clip" "CLIP" false 2 (mds-pi-c))))
(def mds-pi-rs ()
  (mds-wires "PI-RS WIRES"
    (mds-eng-num "clip" "CLIP" false 2 (mds-pi-c))
    (mds-eng-num "dist" "DIST" false 2 (mds-pi-c))
    (mds-eng-blank)))

; TRX-RS: fixed-ratio rim pings off the base note; only pitch and drive matter
(def mds-trx-rs ()
  (mds-engine-column "TRX-RS" (mds-trx-c)
    (mds-eng-num "humanize" "HMNZ" false 2 (mds-trx-c))
    (mds-big-knob "ptch" "PTCH" 0 (mds-trx-c))
    (mds-big-knob "clip" "CLIP" 2 (mds-trx-c))
    (mds-big-knob "dist" "DIST" 2 (mds-trx-c))
    (mds-eng-num "srr" "SRR" false 2 (mds-trx-c))
    (mds-eng-blank)
    (mds-eng-blank)))

; EFM-RS: FM rim; DEC scales its short body envelope
(def mds-efm-rs ()
  (mds-engine-column "EFM-RS" (mds-efm-c)
    (mds-eng-num "mdec" "MDEC" "ms" 0 (mds-efm-c))
    (mds-big-knob "mod_amt" "MOD" 2 (mds-efm-c))
    (mds-big-knob "mfrq" "MFRQ" 0 (mds-efm-c))
    (mds-big-knob "dec" "DEC" 0 (mds-efm-c))
    (mds-eng-num "clip" "CLIP" false 2 (mds-efm-c))
    (mds-eng-num "dist" "DIST" false 2 (mds-efm-c))
    (mds-eng-blank)))

(def mds-engine-panel ()
  (let ((e (mds-engine-index)))
    (if (= e 1) (mds-trx-sd)
      (if (= e 2) (mds-efm-sd)
        (if (= e 3) (mds-pi-sd)
          (if (= e 4) (mds-trx-rs)
            (if (= e 5) (mds-efm-rs)
              (mds-pi-rs))))))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :start
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (mds-machine)
      (mds-noise))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (mds-filter)
      (mds-dirt))
    (subtree :key (str "md-snare-engine-" (mds-engine-index)) (mds-engine-panel))))
