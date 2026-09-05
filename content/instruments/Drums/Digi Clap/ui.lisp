; DigiClap UI — the classic synthesised claps (808, 909, LINN), laid out like
; Digi Snare: two 2-panel columns (MACHINE/BURST, FILTER/DIRT) with a
; full-height engine column on the right that swaps with the machine selector
; and carries that machine's own controls as oversized knobs. No parameter is
; shown in more than one place.
;
; Param names match dsp.lisp. Section titles are plain :fg text; knobs use
; the default cyan accent except the engine column, which takes a colour per
; machine (808 orange, 909 blue, LINN green).

(def dcl-engine-options () '("808" "909" "LINN"))

(def dcl-engine-index ()
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param "engine")))
    (let ((e (if p (round (if (get p :value-field) (reactive-get "SEQ" (get p :value-field)) (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value p)))) 1)))
      (if (= e 2) 2 (if (= e 3) 3 1)))))

(def dcl-num-w () 4.6)
(def dcl-grid-w () (+ (* 2 (dcl-num-w)) 0.18))
(def dcl-knob-w () 4.55)
(def dcl-c () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def dcl-808-c () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def dcl-909-c () (eseq.effects.custom-ui-lego/ui-accent-blue))
(def dcl-linn-c () (eseq.effects.custom-ui-lego/ui-accent-green))

(def dcl-title-w (title w)
  (box :width w :height 0.82 :v-align :end
    (label title :font-size 9.2 :width w :height 0.82 :color :fg :bg :transparent)))
(def dcl-title (title) (dcl-title-w title (dcl-num-w)))
(def dcl-num-w-s (name title unit decimals w color)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title w decimals unit color))
(def dcl-num (name title unit decimals)
  (dcl-num-w-s name title unit decimals (dcl-num-w) (dcl-c)))
(def dcl-knob (name title decimals color)
  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 name title (dcl-knob-w) color decimals))
; same height as a micro-num so a panel with empty grid slots keeps its title
; at the same vertical spot as a full one
(def dcl-blank () (box :width (dcl-num-w) :height 1.84))

; panel skeleton: title + up to 3 pickers in a 2x2 grid on the left, knobs right
(def dcl-panel (title n1 n2 n3 knobs)
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width (dcl-grid-w) :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start (dcl-title title) n1)
        (h-stack :gap 0.18 :align :start n2 n3))
      (box :width :fill :height 0.1)
      knobs)))
(def dcl-knobs-2 (a b) (h-stack :gap 0.10 :align :start a b))
(def dcl-knobs-3 (a b c) (h-stack :gap 0.10 :align :start a b c))

; MACHINE: the machine selector sits where the first picker would, so it reads
; as the panel's headline; base note and humanize below it.
(def dcl-machine ()
  (dcl-panel "MACHINE"
    (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "engine" "MACHINE" (dcl-num-w) (dcl-engine-options) (dcl-c))
          (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 (dcl-num-w) (dcl-c))
          (dcl-num "humanize" "HMNZ" false 2)
    (dcl-knobs-3 (dcl-knob "ptch" "PTCH" 0 (dcl-c))
          (dcl-knob "dec" "DEC" 0 (dcl-c))
          (dcl-knob "level" "LEVEL" 2 (dcl-c)))))

; BURST: the flam that makes a clap a clap — how many bursts, how far apart,
; how fast each one dies — and the noise band they are cut from. SNAP (burst
; level) lives in the engine column so nothing is shown twice.
(def dcl-burst ()
  (dcl-panel "BURST"
    (dcl-num "bursts" "BRST" false 0)
          (dcl-num "sprd" "SPRD" "ms" 0)
          (dcl-num "bdec" "BDEC" "ms" 0)
    (dcl-knobs-2 (dcl-knob "tone" "TONE" 0 (dcl-c))
          (dcl-knob "reso" "RESO" 2 (dcl-c)))))

; FILTER: the MD track filter (HP at FLTF, LP at FLTF+FLTW) plus the EQ band
(def dcl-filter ()
  (dcl-panel "FILTER"
    (dcl-num "fltq" "FLTQ" false 2)
          (dcl-num "eqf" "EQF" "Hz" 0)
          (dcl-num "eqg" "EQG" "dB" 1)
    (dcl-knobs-2 (dcl-knob "fltf" "FLTF" 0 (dcl-c))
          (dcl-knob "fltw" "FLTW" 0 (dcl-c)))))

; DIRT: sample-rate reduction, distortion, and the AM stage
(def dcl-dirt ()
  (dcl-panel "DIRT"
    (dcl-num "amd" "AMD" false 2)
          (dcl-num "amf" "AMF" "Hz" 0)
          (dcl-blank)
    (dcl-knobs-2 (dcl-knob "srr" "SRR" 2 (dcl-c))
          (dcl-knob "dist" "DIST" 2 (dcl-c)))))

; engine column: one full-height column spanning both panel rows, with three
; oversized knobs on a wide pitch so the labels never shrink.
(def dcl-engine-w () 18.0)
(def dcl-engine-h ()
  (+ (* 2 (eseq.effects.custom-ui-lego/ui-lego-dense-h)) (eseq.effects.custom-ui-lego/ui-lego-gap)))
(def dcl-big-knob (name title decimals color)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 name title 5.6 3.12 3.12 color decimals))
(def dcl-eng-num (name title unit decimals color)
  (dcl-num-w-s name title unit decimals 5.6 color))
(def dcl-eng-blank () (box :width 5.6 :height 1.84))
(def dcl-engine-column (title color top k1 k2 k3 b1 b2 b3)
  (eseq.effects.custom-ui-lego/ui-lego-panel-width-s (dcl-engine-w) (dcl-engine-h) 0 :instrument-group-bg
    (box :width :fill :height :fill :v-align :start
      (v-stack :width :fill :gap 0.30 :align :start
        (h-stack :gap 0.22 :align :start (dcl-title title) top)
        (h-stack :gap 0.10 :align :start k1 k2 k3)
        (h-stack :gap 0.22 :align :end b1 b2 b3)))))

; Engine columns carry only what the shared panels do not: every parameter
; is shown exactly once. BODY is the tail level, SNAP the burst level, CLIP the
; per-machine overdrive.

; 808: resonant band, saw-shaped bursts, dry tail.
(def dcl-808 ()
  (dcl-engine-column "808" (dcl-808-c)
    (dcl-eng-blank)
    (dcl-big-knob "body" "BODY" 2 (dcl-808-c))
    (dcl-big-knob "snap" "SNAP" 2 (dcl-808-c))
    (dcl-big-knob "clip" "CLIP" 2 (dcl-808-c))
    (dcl-eng-blank)
    (dcl-eng-blank)
    (dcl-eng-blank)))

; 909: wide bright band, saturated bursts, a lowpassed reverb tail that closes
; as it decays. TLPF is the tail's opening cutoff.
(def dcl-909 ()
  (dcl-engine-column "909" (dcl-909-c)
    (dcl-eng-num "snap" "SNAP" false 2 (dcl-909-c))
    (dcl-big-knob "body" "BODY" 2 (dcl-909-c))
    (dcl-big-knob "tlpf" "TLPF" 0 (dcl-909-c))
    (dcl-big-knob "clip" "CLIP" 2 (dcl-909-c))
    (dcl-eng-blank)
    (dcl-eng-blank)
    (dcl-eng-blank)))

; LINN: sampled-clap crowd. CROWD is the micro-burst density, AIR opens the
; upper band, BITS is the sample word length.
(def dcl-linn ()
  (dcl-engine-column "LINN" (dcl-linn-c)
    (dcl-eng-num "bits" "BITS" false 0 (dcl-linn-c))
    (dcl-big-knob "crowd" "CROWD" 2 (dcl-linn-c))
    (dcl-big-knob "air" "AIR" 2 (dcl-linn-c))
    (dcl-big-knob "body" "BODY" 2 (dcl-linn-c))
    (dcl-eng-num "snap" "SNAP" false 2 (dcl-linn-c))
    (dcl-eng-num "clip" "CLIP" false 2 (dcl-linn-c))
    (dcl-eng-blank)))

(def dcl-engine-panel ()
  (let ((e (dcl-engine-index)))
    (if (= e 2) (dcl-909)
      (if (= e 3) (dcl-linn)
        (dcl-808)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :start
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (dcl-machine)
      (dcl-burst))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (dcl-filter)
      (dcl-dirt))
    (subtree :key (str "digiclap-engine-" (dcl-engine-index)) (dcl-engine-panel))))
