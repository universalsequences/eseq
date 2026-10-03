(def ride-kit-stroke (text)
  (label text :width 11.6 :height 0.56 :v-align :center :font-size 8.5 :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

(def ride-kit-caption (text)
  (label text :width 35.3 :height 0.45 :v-align :center :font-size 8.5
    :color (rgba 0.16 0.06 0.11 1) :bg :transparent))

(def ride-kit-strokes-view ()
  (v-stack :gap 0.15
    (ride-kit-caption "Strokes repeat every octave / C4 octave = recorded pitch")
    (h-stack :gap 0.2
          (v-stack :gap 0.05 (ride-kit-stroke "C  Ride") (ride-kit-stroke "C#  Ride Push") (ride-kit-stroke "D  Ride Soft") (ride-kit-stroke "D#  Ghost"))
          (v-stack :gap 0.05 (ride-kit-stroke "E  Tip") (ride-kit-stroke "F  Dark") (ride-kit-stroke "F#  Shoulder") (ride-kit-stroke "G  Shoulder Accent"))
          (v-stack :gap 0.05 (ride-kit-stroke "G#  Shoulder Soft") (ride-kit-stroke "A  Ping") (ride-kit-stroke "A#  Bell") (ride-kit-stroke "B  Bell Open")))))

(def ride-kit-bind (name) (eseq.effects.physical-model-surface/bind name))

;; Normalized two-pole stick force over 0-1.5 ms, as the DSP computes it.
(defwidget pm-ride-kit-stick
  :width 35.3 :height 3.1 :state (hardness dynamics) :bindable (hardness dynamics)
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
(def ride-kit-stick-view ()
  (v-stack :gap 0.15 (ride-kit-caption "Stick contact force / 0-1.5 ms / faint = half-velocity hit")
    (pm-ride-kit-stick :debug-name "pm-ride-kit-stick" :hardness (ride-kit-bind "stick.hardness")
      :dynamics (ride-kit-bind "stick.dynamics"))))

;; Ring of the plate's typical mode (measured 11.9/s at 1 kHz) with
;; decay and muffle loss; faint = the same mode once choked.
(defwidget pm-ride-kit-ring
  :width 35.3 :height 3.1 :state (decay muffle choke) :bindable (decay muffle choke)
  :shader
  (let ((time (* 3 0.5 (+ 1 (/ x aspect))))
        (rate (+ (/ 11.9 decay) (* 40 muffle muffle)))
        (curve (- 0.8 (* 1.55 (pow 2.7182818 (* -1 rate time)))))
        (held (min time 0.5))
        (choked (- 0.8 (* 1.55 (pow 2.7182818 (- (* -1 rate time) (* 120 choke choke (- time held))))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.14))
      (sdf/paint (- (abs (- y choked)) 0.015) (rgba 0.16 0.06 0.11 0.45))
      (sdf/paint (- (abs (- y curve)) 0.025) (rgba 0.16 0.06 0.11 1)))))
(def ride-kit-ring-view ()
  (v-stack :gap 0.15 (ride-kit-caption "Plate ring / 0-3 s / faint = released at 0.5 s with Choke")
    (pm-ride-kit-ring :debug-name "pm-ride-kit-ring" :decay (ride-kit-bind "cymbal.decay")
      :muffle (ride-kit-bind "cymbal.muffle") :choke (ride-kit-bind "cymbal.choke"))))

;; Stereo scatter: every mode keeps a fixed pan; Width opens the fan.
(defwidget pm-ride-kit-width
  :width 35.3 :height 3.1 :state (amount) :bindable (amount)
  :shader
  (let ((spread (* aspect 0.9 amount))
        (fan (- (abs x) (+ 0.02 (* spread (- 0.8 (* 0.5 (+ y 0.8))))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (rgba 0.24 0.93 0.81 1))
      (sdf/paint (max fan (- y 0.8) (- -0.8 y)) (rgba 0.16 0.06 0.11 0.25))
      (sdf/paint (- (abs (+ y 0.85)) 0.025) (rgba 0.16 0.06 0.11 1)))))
(def ride-kit-width-view ()
  (v-stack :gap 0.15 (ride-kit-caption "Mode scatter across the stereo field / 0 = recorded mono")
    (pm-ride-kit-width :debug-name "pm-ride-kit-width" :amount (ride-kit-bind "output.width"))))

(defsynth-ui
  (eseq.effects.physical-model-surface/panel "PM RIDE KIT"
    (list
      '("STICK" ("stick.hardness" "Hardness" 2 :linear) ("stick.click" "Click" 2 :linear))
      '("RING" ("cymbal.decay" "Decay" 2 :log) ("cymbal.choke" "Choke" 2 :linear))
      '("SPACE" ("output.width" "Width" 2 :linear) ("cymbal.tune" "Tune st" 1 :linear))
      '("LEVEL" ("output.tone_hz" "Tone Hz" 0 :log) ("output.gain" "Output" 2 :linear)))
    (list
      (dict :title "Strokes"
        :view (lambda () (ride-kit-strokes-view))
        :controls (lambda () '(("voice_mode" "Voicing" 0)))
        :hint "Voicing 1 = one cymbal: every hit lands on the same ringing plate.")
      (dict :title "Stick"
        :view (lambda () (ride-kit-stick-view))
        :controls (lambda () '(("stick.hardness" "Hardness" 2) ("stick.dynamics" "Vel > timbre" 2)
                               ("stick.click" "Click" 2) ("stick.click_tone" "Click tone" 2)))
        :hint "Harder sticks shorten contact; energy is redistributed, not added.")
      (dict :title "Ring"
        :view (lambda () (ride-kit-ring-view))
        :controls (lambda () '(("cymbal.decay" "Decay" 2) ("cymbal.muffle" "Muffle" 2) ("cymbal.choke" "Choke" 2) ("cymbal.wash" "Wash" 2)))
        :hint "Muffle is tape on the plate; Choke grabs it when the key is released.")
      (dict :title "Space"
        :view (lambda () (ride-kit-width-view))
        :controls (lambda () '(("output.width" "Width" 2) ("cymbal.tune" "Tune st" 1)))
        :hint "The record is one mono channel; Width scatters the plate's modes.")
      (dict :title "Output"
        :view (lambda () (eseq.effects.physical-model-surface/saron-output-view))
        :controls (lambda () '(("output.drive" "Drive" 2) ("output.tone_hz" "Tone Hz" 0) ("output.gain" "Output" 2)))
        :hint "Drive saturates the whole plate; Tone is a gentle low-pass."))))
