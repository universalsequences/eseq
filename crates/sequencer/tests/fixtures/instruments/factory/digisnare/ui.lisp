; DigiSnare UI — the synthesised Machinedrum snare machines (TRX-SD, EFM-SD,
; EFM-RS), laid out like membrane-snare-mk2: two 2-panel columns
; (MACHINE/NOISE, FILTER/DIRT) with a full-height engine column on the right
; that swaps with the machine selector and carries the three controls that
; matter most for that machine as oversized knobs.
;
; Param names match dsp.lisp. Section titles are plain :fg text; knobs use
; the default cyan accent except the engine column, which takes a colour per
; machine family (TRX orange, EFM blue).

(def dsn-engine-options () '("TRX-SD" "EFM-SD" "EFM-RS"))

(def dsn-engine-index ()
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param "engine")))
    (let ((e (if p (round (if (get p :value-field) (reactive-get "SEQ" (get p :value-field)) (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value p)))) 1)))
      (if (= e 2) 2 (if (= e 3) 3 1)))))

(def dsn-num-w () 4.6)
(def dsn-grid-w () (+ (* 2 (dsn-num-w)) 0.18))
(def dsn-knob-w () 4.55)
(def dsn-c () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def dsn-trx-c () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def dsn-efm-c () (eseq.effects.custom-ui-lego/ui-accent-blue))

(def dsn-title-w (title w)
  (box :width w :height 0.82 :v-align :end
    (label title :font-size 9.2 :width w :height 0.82 :color :fg :bg :transparent)))
(def dsn-title (title) (dsn-title-w title (dsn-num-w)))
(def dsn-num-w-s (name title unit decimals w color)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title w decimals unit color))
(def dsn-num (name title unit decimals)
  (dsn-num-w-s name title unit decimals (dsn-num-w) (dsn-c)))
(def dsn-knob (name title decimals color)
  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 name title (dsn-knob-w) color decimals))
; same height as a micro-num so a panel with empty grid slots keeps its title
; at the same vertical spot as a full one
(def dsn-blank () (box :width (dsn-num-w) :height 1.84))

; panel skeleton: title + up to 3 pickers in a 2x2 grid on the left, knobs right
(def dsn-panel (title n1 n2 n3 knobs)
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width (dsn-grid-w) :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start (dsn-title title) n1)
        (h-stack :gap 0.18 :align :start n2 n3))
      (box :width :fill :height 0.1)
      knobs)))
(def dsn-knobs-2 (a b) (h-stack :gap 0.10 :align :start a b))
(def dsn-knobs-3 (a b c) (h-stack :gap 0.10 :align :start a b c))

; MACHINE: the machine selector sits where the first picker would, so it reads
; as the panel's headline; base note and humanize below it.
(def dsn-machine ()
  (dsn-panel "MACHINE"
    (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "engine" "MACHINE" (dsn-num-w) (dsn-engine-options) (dsn-c))
          (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 (dsn-num-w) (dsn-c))
          (dsn-num "humanize" "HMNZ" false 2)
    (dsn-knobs-3 (dsn-knob "ptch" "PTCH" 0 (dsn-c))
          (dsn-knob "dec" "DEC" 0 (dsn-c))
          (dsn-knob "level" "LEVEL" 2 (dsn-c)))))

; NOISE: the shared snap/noise layer (SNAP+TONE feed TRX-SD, NOISE feeds EFM-SD)
(def dsn-noise ()
  (dsn-panel "NOISE"
    (dsn-num "ndec" "NDEC" "ms" 0)
          (dsn-num "hpf" "HPF" "Hz" 0)
          (dsn-num "tone" "TONE" false 2)
    (dsn-knobs-3 (dsn-knob "snap" "SNAP" 2 (dsn-c))
          (dsn-knob "noise" "NOISE" 2 (dsn-c))
          (dsn-knob "clip" "CLIP" 2 (dsn-c)))))

; FILTER: the MD track filter (HP at FLTF, LP at FLTF+FLTW) plus the EQ band
(def dsn-filter ()
  (dsn-panel "FILTER"
    (dsn-num "fltq" "FLTQ" false 2)
          (dsn-num "eqf" "EQF" "Hz" 0)
          (dsn-num "eqg" "EQG" "dB" 1)
    (dsn-knobs-2 (dsn-knob "fltf" "FLTF" 0 (dsn-c))
          (dsn-knob "fltw" "FLTW" 0 (dsn-c)))))

; DIRT: sample-rate reduction, distortion, and the AM stage
(def dsn-dirt ()
  (dsn-panel "DIRT"
    (dsn-num "amd" "AMD" false 2)
          (dsn-num "amf" "AMF" "Hz" 0)
          (dsn-blank)
    (dsn-knobs-2 (dsn-knob "srr" "SRR" 2 (dsn-c))
          (dsn-knob "dist" "DIST" 2 (dsn-c)))))

; engine column: one full-height column spanning both panel rows, with three
; oversized knobs on a wide pitch so the labels never shrink.
(def dsn-engine-w () 18.0)
(def dsn-engine-h ()
  (+ (* 2 (eseq.effects.custom-ui-lego/ui-lego-dense-h)) (eseq.effects.custom-ui-lego/ui-lego-gap)))
(def dsn-big-knob (name title decimals color)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 name title 5.6 3.12 3.12 color decimals))
(def dsn-eng-num (name title unit decimals color)
  (dsn-num-w-s name title unit decimals 5.6 color))
(def dsn-eng-blank () (box :width 5.6 :height 1.84))
(def dsn-engine-column (title color top k1 k2 k3 b1 b2 b3)
  (eseq.effects.custom-ui-lego/ui-lego-panel-width-s (dsn-engine-w) (dsn-engine-h) 0 :instrument-group-bg
    (box :width :fill :height :fill :v-align :start
      (v-stack :width :fill :gap 0.30 :align :start
        (h-stack :gap 0.22 :align :start (dsn-title title) top)
        (h-stack :gap 0.10 :align :start k1 k2 k3)
        (h-stack :gap 0.22 :align :end b1 b2 b3)))))

; TRX-SD: two-partial analog body; BUMP is the pitch-envelope depth, BENV its
; time, TUNE spreads the second partial.
(def dsn-trx-sd ()
  (dsn-engine-column "TRX-SD" (dsn-trx-c)
    (dsn-eng-num "benv" "BENV" "ms" 0 (dsn-trx-c))
    (dsn-big-knob "bump" "BUMP" 0 (dsn-trx-c))
    (dsn-big-knob "tone" "TONE" 2 (dsn-trx-c))
    (dsn-big-knob "clip" "CLIP" 2 (dsn-trx-c))
    (dsn-eng-num "tune" "TUNE" false 2 (dsn-trx-c))
    (dsn-eng-num "snap" "SNAP" false 2 (dsn-trx-c))
    (dsn-eng-blank)))

; EFM-SD: FM body; MOD is the index, MFRQ/MDEC the modulator pitch and decay
(def dsn-efm-sd ()
  (dsn-engine-column "EFM-SD" (dsn-efm-c)
    (dsn-eng-num "mdec" "MDEC" "ms" 0 (dsn-efm-c))
    (dsn-big-knob "mod_amt" "MOD" 2 (dsn-efm-c))
    (dsn-big-knob "mfrq" "MFRQ" 0 (dsn-efm-c))
    (dsn-big-knob "noise" "NOISE" 2 (dsn-efm-c))
    (dsn-eng-num "ndec" "NDEC" "ms" 0 (dsn-efm-c))
    (dsn-eng-num "hpf" "HPF" "Hz" 0 (dsn-efm-c))
    (dsn-eng-num "clip" "CLIP" false 2 (dsn-efm-c))))

; EFM-RS: FM rim; DEC scales its short body envelope
(def dsn-efm-rs ()
  (dsn-engine-column "EFM-RS" (dsn-efm-c)
    (dsn-eng-num "mdec" "MDEC" "ms" 0 (dsn-efm-c))
    (dsn-big-knob "mod_amt" "MOD" 2 (dsn-efm-c))
    (dsn-big-knob "mfrq" "MFRQ" 0 (dsn-efm-c))
    (dsn-big-knob "dec" "DEC" 0 (dsn-efm-c))
    (dsn-eng-num "clip" "CLIP" false 2 (dsn-efm-c))
    (dsn-eng-num "dist" "DIST" false 2 (dsn-efm-c))
    (dsn-eng-blank)))

(def dsn-engine-panel ()
  (let ((e (dsn-engine-index)))
    (if (= e 2) (dsn-efm-sd)
      (if (= e 3) (dsn-efm-rs)
        (dsn-trx-sd)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :start
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (dsn-machine)
      (dsn-noise))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (dsn-filter)
      (dsn-dirt))
    (subtree :key (str "digisnare-engine-" (dsn-engine-index)) (dsn-engine-panel))))
