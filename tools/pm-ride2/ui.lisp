(def ride-row (text)
  (label text :width 17.4 :height 0.56 :v-align :center :font-size 8.5 :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

(def ride-caption (text)
  (label text :width 35.3 :height 0.45 :v-align :center :font-size 8.5
    :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

(def ride-cymbals-view ()
  (v-stack :gap 0.15
    (ride-caption "Character / six measured rides, dark to bright; never blended")
    (h-stack :gap 0.2
          (v-stack :gap 0.05 (ride-row "0  Dark") (ride-row "1  Warm") (ride-row "2  Classic"))
          (v-stack :gap 0.05 (ride-row "3  Dry") (ride-row "4  Bright") (ride-row "5  Crisp")))))

(def ride-bind (name) (eseq.effects.physical-model-surface/bind name))

;; Normalized two-pole stick force over 0-1.5 ms, as the DSP computes it.
(defwidget pm-ride-stick
  :width 35.3 :height 3.1 :state (hardness dynamics)
  :shader
  (let ((time (* 0.0015 0.5 (+ 1 (/ x aspect))))
        (hard (* 0.00025 (pow 2 (* 3 (- 0.5 hardness)))))
        (soft (* hard (+ 1 (* 0.75 dynamics))))
        (u (/ time hard)) (v (/ time soft))
        (curve (- 0.8 (* 1.5 u (pow 2.7182818 (- 1 u)))))
        (quiet (- 0.8 (* 1.5 (/ hard soft) v (pow 2.7182818 (- 1 v))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.14))
      (sdf/paint (- (abs (- y quiet)) 0.018) (rgba 0.16 0.06 0.11 0.4))
      (sdf/paint (- (abs (- y curve)) 0.025) (rgba 0.16 0.06 0.11 1)))))
(def ride-stick-view ()
  (v-stack :gap 0.15 (ride-caption "Stick contact force / 0-1.5 ms / faint = half-velocity hit")
    (pm-ride-stick :debug-name "pm-ride-stick" :hardness (ride-bind "stick.hardness")
      :dynamics (ride-bind "stick.dynamics"))))

;; Ring of a typical mode (measured 4.34/s) with decay and muffle loss;
;; faint = the same mode once choked.
(defwidget pm-ride-ring
  :width 35.3 :height 3.1 :state (decay muffle choke)
  :shader
  (let ((time (* 3 0.5 (+ 1 (/ x aspect))))
        (rate (+ (/ 4.34 decay) (* 40 muffle muffle)))
        (curve (- 0.8 (* 1.55 (pow 2.7182818 (* -1 rate time)))))
        (held (min time 0.5))
        (choked (- 0.8 (* 1.55 (pow 2.7182818 (- (* -1 rate time) (* 120 choke choke (- time held))))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.14))
      (sdf/paint (- (abs (- y choked)) 0.015) (rgba 0.16 0.06 0.11 0.45))
      (sdf/paint (- (abs (- y curve)) 0.025) (rgba 0.16 0.06 0.11 1)))))
(def ride-ring-view ()
  (v-stack :gap 0.15 (ride-caption "Plate ring / 0-3 s / faint = released at 0.5 s with Choke")
    (pm-ride-ring :debug-name "pm-ride-ring" :decay (ride-bind "ring.decay")
      :muffle (ride-bind "ring.muffle") :choke (ride-bind "ring.choke"))))

;; Stereo scatter: every mode keeps a fixed pan; Width opens the fan.
(defwidget pm-ride-width
  :width 35.3 :height 3.1 :state (amount)
  :shader
  (let ((spread (* aspect 0.9 amount))
        (fan (- (abs x) (+ 0.02 (* spread (- 0.8 (* 0.5 (+ y 0.8))))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max fan (- y 0.8) (- -0.8 y)) (rgba 0.16 0.06 0.11 0.25))
      (sdf/paint (- (abs (+ y 0.85)) 0.025) (rgba 0.16 0.06 0.11 1)))))
(def ride-width-view ()
  (v-stack :gap 0.15 (ride-caption "Mode scatter across the stereo field / 0 = recorded mono")
    (pm-ride-width :debug-name "pm-ride-width" :amount (ride-bind "output.width"))))

(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM RIDE"
    (list
      '("CYMBAL" ("cymbal.character" "Character" 0 :linear) ("cymbal.bell" "Bell" 2 :linear))
      '("STICK" ("stick.hardness" "Hardness" 2 :linear) ("stick.click" "Click" 2 :linear))
      '("RING" ("ring.decay" "Decay" 2 :log) ("ring.choke" "Choke" 2 :linear))
      '("LEVEL" ("output.width" "Width" 2 :linear) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Cymbal"
        :view (lambda () (ride-cymbals-view))
        :controls (lambda () '(("cymbal.character" "Character" 0) ("cymbal.bell" "Bell" 2) ("cymbal.size" "Size" 2)
                               ("cymbal.tune" "Tune st" 1) ("cymbal.tracking" "Tracking" 2) ("voice_mode" "Voicing" 0)))
        :hint "Voicing 1 = one cymbal: every hit lands on the same ringing plate.")
      (dict :title "Stick"
        :view (lambda () (ride-stick-view))
        :controls (lambda () '(("stick.hardness" "Hardness" 2) ("stick.dynamics" "Vel > timbre" 2)
                               ("stick.click" "Click" 2) ("stick.click_tone" "Click tone" 2)))
        :hint "Harder sticks shorten contact; energy is redistributed, not added.")
      (dict :title "Ring"
        :view (lambda () (ride-ring-view))
        :controls (lambda () '(("ring.decay" "Decay" 2) ("ring.muffle" "Muffle" 2) ("ring.choke" "Choke" 2) ("ring.wash" "Wash" 2)))
        :hint "Muffle is tape on the plate; Choke grabs it when the key is released.")
      (dict :title "Space"
        :view (lambda () (ride-width-view))
        :controls (lambda () '(("output.width" "Width" 2)))
        :hint "The recordings are mono; Width scatters the plate's modes.")
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/saron-output-view))
        :controls (lambda () '(("output.drive" "Drive" 2) ("output.tone_hz" "Tone Hz" 0) ("output.gain" "Output" 2)))
        :hint "Drive saturates the whole plate; Tone is a gentle low-pass."))))
