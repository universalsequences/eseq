;; Arpeggiator panel: one surface. A screen across the top previews a bar of
;; the note order the settings produce (direction x octaves; bar height =
;; pitch, width = gate, strength = velocity), segmented chips pick direction and octave span, and
;; rate / gate / velocity sit along the bottom. Built from the shared lego kit
;; so p-locks, modulation rings and the param plumbing behave like every
;; other effect panel.
(def arp-rate-labels
  (list "1" "1/2" "1/4" "1/8" "1/16" "1/32" "1/64"
        "1/2T" "1/4T" "1/8T" "1/16T" "1/32T" "1/64T"))

(def arp-direction-labels
  (list "UP" "DOWN" "UP-DOWN" "RANDOM"))

(def arp-accent () (eseq.effects.custom-ui-lego/ui-accent-blue))

(def arp-p (name) (eseq.effects.custom-ui-runtime/custom-ui-current-param name))

;; A param's value as an index from its minimum.
(def arp-index (p)
  (round (- (eseq.effects.custom-ui-runtime/custom-ui-param-value p) (get p :min))))

(def arp-label (text width)
  (label text :width width :height 0.45 :font-size 8.2 :v-align :center :color :dim :bg :transparent))

;; ── Preview screen ──

;; Pitch rank of each arp step for `notes` held notes: up climbs, down
;; falls, up-down climbs then falls without repeating the ends, random is a
;; fixed shuffle (the real order changes every cycle).
(def arp-order (direction notes)
  (let ((up (range notes)))
    (if (= direction 1) (reverse up)
      (if (= direction 2)
        (append up (map (lambda (i) (- notes 2 i)) (range (- notes 2))))
        (if (= direction 3)
          (map (lambda (i) (mod (* i 7) notes)) up)
          up)))))

;; A four-note chord per octave, played across a bar of sixteen steps.
(def arp-preview-notes (octaves) (* 4 octaves))
(def arp-preview-steps () 16)

(def arp-screen-w () 23.0)
(def arp-bars-h () 2.7)

(def arp-bar (rank notes slot-w gate velocity)
  (let ((h (* (arp-bars-h) (+ 0.18 (* 0.82 (/ (+ rank 1) notes))))))
    (box :width slot-w :height (arp-bars-h) :v-align :end :padding-right 0.06
      (box :width (* (- slot-w 0.06) gate) :height h :corner-radius 1
        :background-color (rgba 0 0 0 (+ 0.35 (* 0.65 velocity)))))))

(def arp-screen ()
  (let ((rate (arp-p "rate")) (dir (arp-p "direction")) (oct (arp-p "octave"))
        (gate (arp-p "gate")) (vel (arp-p "velocity")))
    (if (and rate dir oct gate vel)
      (let ((di (arp-index dir))
            (octaves (round (eseq.effects.custom-ui-runtime/custom-ui-param-value oct)))
            (g (eseq.effects.custom-ui-runtime/custom-ui-param-value gate))
            (v (eseq.effects.custom-ui-runtime/custom-ui-param-value vel))
            (notes (arp-preview-notes octaves))
            (order (arp-order di notes))
            (steps (range (arp-preview-steps)))
            (slot-w (/ (- (arp-screen-w) 0.4) (arp-preview-steps))))
        (subtree :key (str "arp-screen-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
                           "-" (arp-index rate) "-" di "-" octaves "-" g "-" v)
          (v-stack :width (arp-screen-w) :gap 0
            (box :width (arp-screen-w) :height 0.62 :background-color :black
              (label (str "ARP / " (nth arp-direction-labels di) " / "
                          octaves (if (= octaves 1) " OCT / " " OCTS / ")
                          (nth arp-rate-labels (arp-index rate)))
                :width (arp-screen-w) :height 0.62 :h-align :center :v-align :center
                :font-size 8 :color (arp-accent) :bg :transparent))
            (box :width (arp-screen-w) :height (+ (arp-bars-h) 0.4) :padding 0.2
              :background-color (arp-accent)
              (h-stack :gap 0 :align :end
                (each steps (lambda (step i) (arp-bar (nth order (mod step (len order))) notes slot-w g v))))))))
      (label "missing: arp params" :font-size 8 :color :red :bg :transparent))))

;; ── Segmented chips ──

;; One chip per option of param `name`; the current one is accent-filled.
;; Keyed by the value, which it reads concretely, so a change rebuilds it.
(def arp-segments (name labels chip-w)
  (let ((p (arp-p name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (if p
      (let ((current (arp-index p))
            (indices (range (len labels))))
        (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "arp-seg-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
          (subtree :key (str "arp-seg-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name "-" current)
            (h-stack :gap 0.12
              (each indices
                (lambda (i n)
                  (button (nth labels i)
                    :width chip-w :height 0.92 :padding 0 :font-size 8.6
                    :corner-radius 3
                    :border-color :transparent
                    :background-color (if (= i current) (arp-accent) :instrument-control-bg)
                    :color (if (= i current) :black :dim)
                    :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
                    :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
                    :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
                    :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
                    :on-click (lambda (x y r)
                      (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
                        (+ (get p :min) i))))))))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

;; Rate dropdown in the theme's plain dropdown colors: an accent-colored
;; value reads as a link, not a setting.
(def arp-rate (width)
  (let ((p (arp-p "rate"))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (if p
      (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "arp-rate-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name))
        (subtree :key (str "arp-rate-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name))
          (arp-labeled "rate" width
            (dropdown :value-index (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
              :value-index-offset (get p :min)
              :options arp-rate-labels
              :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
              :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
              :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
              :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
              :width width :height 0.92 :font-size 8.6
              :on-change (lambda (v)
                (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
                  (+ (get p :min) (eseq.effects.param-controls/custom-ui-option-index arp-rate-labels v))))))))
      (label "missing: rate" :font-size 8 :color :red :bg :transparent))))

(def arp-labeled (title width body)
  (v-stack :width width :gap 0.08 :align :start
    (arp-label title width)
    body))

(def-midi-fx-ui
  (box :width (eseq.effects.custom-ui-lego/ui-lego-col-w)
       :height (eseq.effects.custom-ui-lego/ui-lego-full-h)
       :background-color :instrument-group-bg
       :corner-radius 7
       :padding 0.3
    (v-stack :width :fill :gap 0.3 :align :start
      (arp-screen)
      (h-stack :gap 0.9 :align :start
        (arp-labeled "direction" 12.3
          (arp-segments "direction" (list "up" "down" "up/dn" "rand") 2.98))
        (arp-labeled "octaves" 9.8
          (arp-segments "octave" (list "1" "2" "3" "4") 2.36)))
      (h-stack :gap 0.5 :align :start
        (arp-rate 6.0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "gate" "gate" 4.7 (arp-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "velocity" "vel" 4.7 (arp-accent) 2)))))
