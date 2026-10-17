;; Plaits — the module's front panel as three surfaces: the model and its
;; three timbral controls (with what HARMONICS / TIMBRE / MORPH do in the
;; selected model), the internal envelope's FM / TIMBRE / MORPH amounts, and
;; the low-pass gate (mode, decay, colour) with the OUT/AUX blend. Every
;; control appears exactly once.

(def pl-knob () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def pl-head () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def pl-text () :fg)

(def pl-surf () :instrument-group-bg)
(def pl-bord () :border-inactive)

(def pl-panel-dense (section body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-dense-h) (pl-surf) (pl-bord) false body))
(def pl-small-row (body)
  (box :width :fill :height :fill :v-align :center body))

;; UI order: the green bank, the red bank, then the 1.2 firmware bank.
(def pl-model-options ()
  '("VA" "Waveshaper" "2-op FM" "Formant/PD" "Harmonic" "Wavetable" "Chords" "Speech"
    "Swarm" "Noise" "Particle" "String" "Modal" "Bass drum" "Snare" "Hi-hat"
    "VA + VCF" "Phase dist" "6-op FM 1" "6-op FM 2" "6-op FM 3" "Wave terrain" "String mach" "Chiptune"))

;; Per model: HARMONICS, TIMBRE, MORPH, AUX.
(def pl-model-help ()
  '(("detune" "square: pulse > sync" "saw: tri > notch" "aux: second oscillator")
    ("waveshaper curve" "fold amount" "asymmetry" "aux: other fold curve")
    ("frequency ratio" "FM index" "feedback" "aux: sub-octave")
    ("formant ratio" "formant freq" "formant width" "aux: filtered-wave sim")
    ("spectral bumps" "main harmonic" "bump shape" "aux: drawbar organ")
    ("bank" "row" "column" "aux: 5-bit lo-fi")
    ("chord type" "inversion" "waveform" "aux: root note")
    ("synth > word bank" "species" "phoneme / segment" "aux: raw source")
    ("pitch scatter" "grain density" "grain duration" "aux: sine swarm")
    ("filter LP > BP > HP" "clock rate" "resonance" "aux: dual band-pass")
    ("freq scatter" "density" "filter: allpass > BP" "aux: raw dust")
    ("inharmonicity" "excitation" "decay" "aux: raw exciter")
    ("material" "excitation" "decay" "aux: raw exciter")
    ("attack / drive" "brightness" "decay" "aux: synthetic kick")
    ("tone / noise" "drum modes" "decay" "aux: synthetic snare")
    ("metal / noise" "high-pass" "decay" "aux: ring-mod metal")
    ("resonance / character" "cutoff" "waveform / sub" "aux: high-pass, clipped")
    ("distortion freq" "distortion amt" "asymmetry" "aux: free-running PD")
    ("patch" "mod level" "env stretch" "aux: same as out")
    ("patch" "mod level" "env stretch" "aux: same as out")
    ("patch" "mod level" "env stretch" "aux: same as out")
    ("terrain" "path radius" "path offset" "aux: sine-folded path")
    ("chord" "chorus / filter" "waveform" "aux: stereo right")
    ("chord" "arp / inversion" "pulse width" "aux: triangle bass")))

(def pl-help-entry ()
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param "model")))
    (if p
      (nth (pl-model-help) (max 0 (min 23 (round (eseq.effects.custom-ui-runtime/custom-ui-param-value p)))))
      (nth (pl-model-help) 0))))

;; The 6-op models' DX7 patch names (fm_patches_table banks 0..2, as
;; selected by HARMONICS through HysteresisQuantizer2(32) on h * 1.02).
(def pl-dx-names ()
  '(("SOLID BASS" "Mooger Low" "LeaderTape" "MORHOL TB1" "BASS 3" "BILL BASS" "BASS 1" "ELEC BASS" "S.BAS 27.7" "RESONANCES" "SYN-BASS 2" "PRC SYNTH1" "CROMA 2" "ANALOG 4" "ANALOG A" "ANALOG 6" "CS 80" "INSERT 1" "SPIRAL" "DX-TROTT" "GASHAUS" "RING DING" "PAPAGAYO" "WINEGLASS" "AMYTAL" "FAIRLIGHT" "*PPG*Vol.1" "*PPG*Vol.2" "*Fairl. 3" "*Vocoder 2" "*Sequence" "Bounce 4")
    ("E.PIANO 1" "FENDER 1" "WINTRHODES" "RS-EP C" "*Mark III" "CLAV-E.PNO" "SYN-CLAV" "CLAVINET" "PIANO 5" "GRD PIANO1" "STEINWAY" "GUIT ACOUS" "SITAR" "KOTO" "HARPSICH 1" "CLAV 3" "XYLOPHONE" "MARIMBA" "VIBE 1" "GLOKENSPL" "BELL C" "BELLS" "TUB BELLS" "GONG 2" "KETTLE 6" "MID DRM 3" "ORI DRUM 1" "WOOD 6" "LATN DRM 4" "CIMBAL" "SYNDM 25.8" "B.DRM-SNAR")
    ("CLICK 124" "*Hammond 1" "E.ORGAN 3" "60-S ORGAN" "OPTIC 28" "PIPES 1" "PIPES 3" "PIPES 2" "JX-33-P" "SOUNDTRACK" "ICE PAD 2" "M1 PADS" "CARLOS 2" "SOFT TOUCH" "*Planets" "CIRRUS" "ENTRIX" "MAL POLY" "Textures 6" "Etherial5a" "Airy" "BORON A" "VANGELIS 1" "STRINGS C" "STRINGS 3" "STRINGS 2" "STRINGS 7" "FULL STRIN" "SYN-ORCH" "BRASS 1" "BRASS 6 BC" "BR TRUMPET")))

(def pl-value (name fallback)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (if p (eseq.effects.custom-ui-runtime/custom-ui-param-value p) fallback)))

;; HARMONICS line: the DX7 patch name on the 6-op models.
(def pl-help-text (index)
  (let ((m (max 0 (min 23 (round (pl-value "model" 0))))))
    (if (and (= index 0) (>= m 18) (<= m 20))
      (str "patch " (nth (nth (pl-dx-names) (- m 18))
                         (max 0 (min 31 (round (- (* (pl-value "harmonics" 0.5) 1.02 32) 0.5))))))
      (nth (pl-help-entry) index))))

(def pl-help-line (index prefix)
  (label (str prefix (pl-help-text index)) :font-size 10 :color :dim :bg :transparent))

(def pl-main-h () (+ (eseq.effects.custom-ui-lego/ui-lego-dense-h) (eseq.effects.custom-ui-lego/ui-lego-dense-h) (eseq.effects.custom-ui-lego/ui-lego-gap)))
(def pl-main-w () (eseq.effects.custom-ui-lego/ui-lego-wide-col-w))

(def pl-main-block ()
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s 0 (pl-main-w) (pl-main-h) (pl-surf) (pl-bord) false
    (v-stack :width :fill :height :fill :gap 0.30 :align :start :padding 0.12
      (h-stack :gap 0.22 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "PLAITS" 4.4 (pl-head))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "model" "model" 10.0 (pl-model-options) (pl-text)))
      (h-stack :width :fill :gap 0.40 :align :center
        (h-stack :gap 0.30 :align :center
          (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "harmonics" "harmonics" 5.0 4.2 3.8 (pl-knob) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "timbre" "timbre" 5.0 4.2 3.8 (pl-knob) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "morph" "morph" 5.0 4.2 3.8 (pl-knob) 2))
        (v-stack :gap 0.10 :align :start
          (pl-help-line 0 "H: ")
          (pl-help-line 1 "T: ")
          (pl-help-line 2 "M: ")
          (pl-help-line 3 ""))))))

(def pl-voice-block ()
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s 3 (pl-main-w) (eseq.effects.custom-ui-lego/ui-lego-small-h) (pl-surf) (pl-bord) false
    (pl-small-row
      (h-stack :gap 0.22 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 3 "VOICE" 3.6 (pl-head))
        (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 3 5.0 (pl-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "tune" "tune" 5.0 1 "st" (pl-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "level_db" "level" 5.0 1 "dB" (pl-text))))))

;; How far each note's decay envelope sweeps pitch, timbre and morph (the
;; module's attenuverters with TRIG patched).
(def pl-env-block ()
  (pl-panel-dense 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 1 "ENV MOD" 4.4 (pl-head))
        (label "per-note sweep" :font-size 9.5 :color :dim :bg :transparent))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "fm_amt" "fm" 3.7 (pl-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "timbre_amt" "timbre" 3.7 (pl-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "morph_amt" "morph" 3.7 (pl-knob) 2)))))

(def pl-lpg-block ()
  (pl-panel-dense 2
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "LPG" 4.0 (pl-head))
        (h-stack :gap 0.30 :align :center
          (label "mode" :font-size 9.0 :width 2.4 :color (pl-text) :bg :transparent)
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "lpg_mode" "lpg_mode" 5.8 '("ping" "gate") (pl-text))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "decay" "decay" 3.7 (pl-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "lpg" "colour" 3.7 (pl-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "aux_mix" "out/aux" 3.7 (pl-knob) 2)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.05 :align :stretch
    (v-stack :width (pl-main-w) :gap (eseq.effects.custom-ui-lego/ui-lego-gap)
      (pl-main-block)
      (pl-voice-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (pl-env-block)
      (pl-lpg-block))))
