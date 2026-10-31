;; A Fender Precision Bass, built from finite-element solids joined at named
;; points, played fingerstyle through a bass amp.
;;
;;   body   solid alder, 44 mm deep, 330 mm across the lower bout: the
;;          offset double cutaway with its long bass-side horn, the forearm
;;          contour on the top and the belly cut on the back, and the three
;;          routes that are its only hollows: the neck pocket (63.5 mm wide,
;;          98 mm long, 16.5 mm deep), the split-coil pickup's two offset
;;          slots (19 mm deep) and the control cavity under the knobs
;;   neck   one-piece maple ('57 style: the fretboard is the neck), bolted
;;          into the pocket at its heel: a C-shaped back tapering from
;;          44.5 mm across at the nut to 63.5 mm at the heel, and the big
;;          flat Fender headstock. A bolt-on is clamped by four screws, a
;;          softer joint than the hollow body's glued set neck
;;   string four roundwound strings, 45-65-85-105, on a 34-inch (864 mm)
;;          scale. A note is played where a bassist plays it in first
;;          position: E1-G#1 on the E string, A1-C#2 on the A, D2-F#2 on
;;          the D, G2 and up on the G (to its 24th fret). Fretting shortens
;;          the string, so its length follows the note: an octave up is
;;          half the string. Its stiffness is its core's (the wraps add
;;          mass, not bending): 0.81-0.71-0.64-0.56 mm cores give the
;;          stretched upper partials of real bass strings, B = 3.2e-4 on
;;          the open E down to 7e-5 on the open G, and growing up the neck
;;          as the speaking length shortens
;;   finger the first two fingers' flesh, plucking 165 mm from the bridge,
;;          just neckward of the pickup
;;   pickup the split coil: the E/A half 146 mm from the bridge saddles,
;;          the D/G half 127 mm (they are offset). Each note is heard by
;;          the half under its string, at its distance from the bridge on
;;          the fretted string. Fender's 2.4-3.2 mm pole height; a P coil
;;          loaded by the pots and a cable resonates near 3 kHz
;;   amp    a clean valve bass amp into a 15-inch cabinet: an input and a
;;          cone that reach an octave below a guitar amp's, so the low E's
;;          fundamental survives, and a cab falling away above 4 kHz
;;
;; The strings' ends are mounted on the body (the bridge) and the neck (the
;; nut) as on the hollow body: one way (m/mount :back is not yet
;; energy-conserving, eseq-lojj.22), so the body and neck carry the strings
;; but do not ring back into them yet. Once they do, this neck's modes are
;; where a Fender bass's dead spot comes from (the G string around the 5th
;; to 7th fret, where the neck's bending mode soaks up the note).
;;
;; Masses are relative to the string's modal mass (1). A bass string is far
;; heavier than a guitar's (the E is 30 g against a guitar E's 6), so the
;; 2.2 kg body (as solved) is 150 string masses at the open E, not the
;; hollow body's 300; the 0.6 kg neck (without its heavy tuners: a solid is
;; one material) 40.
;;
;; Both solids are wood with a :grain running along the instrument (along
;; x), the growth rings flat (the body is a flatsawn slab, z up through
;; it), so they are orthotropic. Alder is lighter and softer than maple and
;; damps a little less; the finished body's 0.02 adds its finish, pickguard
;; and hardware, the bolted neck's 0.02 its frets and joint. The cells are
;; flat (:cell-size, x y z): 8 mm across and 5.5 mm through the body (eight
;; layers in its depth, five under the pocket), 6 x 4 x 3 mm through the
;; neck. Solving them takes a few minutes the first time (then cached).
;;
;; x runs along the bass toward the headstock, y across it (bass side +y),
;; z up out of the top. The body's origin is the bridge saddles' line on
;; its centre, halfway through its depth (its top at z = 0.022); the neck's
;; is the end of its heel, on the fretboard's surface (z = 0), the nut at
;; x = 0.62. The 'joint spring seats the neck's :heel on the body's
;; :neck-joint (the pocket floor), which puts the nut 864 mm from the
;; saddles; open the script in the Shape Lab and pick Instrument to see it.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

;; ── where each note is played ───────────────────────────────────────────────

(def scale 0.864)
;; the open strings, MIDI notes: E1 A1 D2 G2
(def open-strings (list 28 33 38 43))
;; the notes the curves below run over: the open E to the G string's 24th fret
(def bass-notes (range 28 68))

;; Which string plays MIDI note n (0 E .. 3 G): first position, then up the G.
(def string-of (n) (if (< n 33) 0 (if (< n 38) 1 (if (< n 43) 2 3))))
(def note-hz (n) (* 440 (pow 2 (/ (- n 69) 12))))
;; the speaking length of note n, from the bridge saddle to the fret, m
(def fretted (n) (* scale (pow 2 (/ (- (nth open-strings (string-of n)) n) 12))))
;; the point on note n's string d metres from the bridge (1 is the bridge)
(def from-bridge (n d) (- 1 (/ d (fretted n))))

;; A value per note, as an m/by-note through every note's own value.
;;   f  MIDI note -> value
(def per-note (f) (apply m/by-note (apply append (map (lambda (n) (list (note-hz n) (f n))) bass-notes))))

;; The core radius the string model needs for a wound string: its bending
;; is the core's, d^4 E pi / 64, but its mass is the whole string's, so the
;; equivalent solid steel string's radius is d^2 sqrt(pi rho / (16 mu)).
;; Cores (m) and the strings' masses per metre (kg/m: 40 lb each at pitch).
(def cores (list 0.00081 0.00071 0.00064 0.00056))
(def weights (list 0.0351 0.0197 0.01105 0.00621))
(def core-radius-mm (k)
  (* 1000 (pow (nth cores k) 2) (sqrt (/ (* 3.14159265 (get mat/steel :density)) (* 16 (nth weights k))))))

;; the split coil: the E/A half and the D/G half, metres from the bridge
(def coil-from-bridge (n) (if (< (string-of n) 2) 0.146 0.127))

(m/patch :name "modal-p-bass" :voicing :poly
  :objects (list (m/finger 'fin :hz 350 :damp 0.6 :stiff 1.5 :start 0.1 :depth 0.25 :stroke-ms 18)
                 (m/string 'str :made-of mat/steel :modes 80
                           :length (per-note fretted)
                           :radius-mm (per-note (lambda (n) (core-radius-mm (string-of n))))
                           :const-loss 0.4 :freq-loss 0.25 :mute 25)
                 (m/solid 'body :made-of mat/alder :grain (list :x :z) :modes 48 :loss 2 :hf-loss 0 :damping 0.02 :mass 150
                          :resolution (dict :cell-size (list 0.008 0.008 0.0055))
                          :shape (dict :difference (list (dict :union (list ;; the lower bout
                                                                            (dict :cylinder (dict :axis :z :center (list 0.06 0 0) :height 0.044 :radius 0.165))
                                                                            ;; the upper bout
                                                                            (dict :cylinder (dict :axis :z :center (list 0.27 0 0) :height 0.044 :radius 0.14))
                                                                            ;; the long bass-side horn and the treble horn
                                                                            (dict :cylinder (dict :axis :z :center (list 0.375 0.085 0) :height 0.044 :radius 0.035))
                                                                            (dict :cylinder (dict :axis :z :center (list 0.34 -0.085 0) :height 0.044 :radius 0.03))))
                                                         ;; the waist
                                                         (dict :cylinder (dict :axis :z :center (list 0.175 0.215 0) :height 0.06 :radius 0.1))
                                                         (dict :cylinder (dict :axis :z :center (list 0.175 -0.215 0) :height 0.06 :radius 0.1))
                                                         ;; between the horns, where the neck leaves the body
                                                         (dict :box (dict :center (list 0.43 0 0) :size (list 0.176 0.11 0.06)))
                                                         ;; the treble horn ends short of the bass horn
                                                         (dict :box (dict :center (list 0.45 -0.11 0) :size (list 0.18 0.11 0.06)))
                                                         ;; the forearm contour, bevelling the top's bass edge
                                                         (dict :cylinder (dict :axis :x :center (list 0.03 0.18 0.12) :height 0.2 :radius 0.117))
                                                         ;; the belly cut, under the treble side of the back
                                                         (dict :cylinder (dict :axis :x :center (list 0.17 -0.165 -0.12) :height 0.24 :radius 0.116))
                                                         ;; the neck pocket
                                                         (dict :box (dict :center (list 0.31 0 0.01375) :size (list 0.1328 0.0635 0.0165)))
                                                         ;; the split coil's two slots, the E/A half nearer the neck
                                                         (dict :box (dict :center (list 0.146 0.021 0.0125) :size (list 0.032 0.054 0.019)))
                                                         (dict :box (dict :center (list 0.127 -0.021 0.0125) :size (list 0.032 0.054 0.019)))
                                                         ;; the control cavity, under the volume and tone knobs
                                                         (dict :box (dict :center (list 0 -0.105 0.006) :size (list 0.11 0.05 0.032)))))
                          :points (dict :back (list 0.06 0 -0.022)
                                        :bridge (list 0 0 0.022)
                                        :lower-horn (list 0.345 -0.085 0.022)
                                        :neck-joint (list 0.2936 0 0.0055)
                                        :pickguard (list 0.08 -0.07 0.022)
                                        :tail (list -0.105 0 0)
                                        :upper-horn (list 0.39 0.09 0.022)))
                 (m/solid 'neck :made-of mat/maple :grain (list :x :z) :modes 32 :loss 2 :hf-loss 0 :damping 0.02 :mass 40
                          :resolution (dict :cell-size (list 0.006 0.004 0.003))
                          :shape (dict :difference (list (dict :union (list ;; the shaft: a cone about an axis 3 mm above
                                                                            ;; the fretboard, cut flat there, so it is
                                                                            ;; 44.5 mm across and 20 mm deep at the nut
                                                                            ;; and 61 x 28 at the heel
                                                                            (dict :revolve (dict :axis :x :center (list 0 0 0.003)
                                                                                                 :profile (list (list 0 0.09) (list 0.0305 0.09)
                                                                                                                (list 0.0224 0.625) (list 0 0.625))))
                                                                            ;; the heel, square in the pocket
                                                                            (dict :box (dict :center (list 0.0525 0 -0.0135) :size (list 0.105 0.0635 0.027)))
                                                                            ;; the headstock, its face 3 mm under the
                                                                            ;; fretboard and widened on the bass side
                                                                            ;; for the four tuners in a row
                                                                            (dict :box (dict :center (list 0.7175 0.012 -0.0105) :size (list 0.195 0.088 0.015)))))
                                                         ;; the fretboard's flat top
                                                         (dict :box (dict :center (list 0.35 0 0.05) :size (list 0.8 0.1 0.1)))))
                          :points (dict :fret-5 (list 0.4083 0 0)
                                        :fret-7 (list 0.3333 0 0)
                                        :fret-12 (list 0.188 0 0)
                                        :headstock (list 0.8 0.03 -0.003)
                                        :heel (list 0.0525 0 -0.027)
                                        :nut (list 0.62 0 0))))
  :connections (list (m/pluck 'plk 'fin 'str (per-note (lambda (n) (from-bridge n 0.165))))
                     ;; the string's ends: the bridge saddle (end 1) on the
                     ;; body, the nut (end 0) on the neck. A fretted note's
                     ;; end is its fret, but a mount's point is fixed, so it
                     ;; stays on the nut
                     (m/mount 'bridge 'body :bridge 'str 1)
                     (m/mount 'nut 'neck :nut 'str 0)
                     ;; the bolt-on joint: the heel clamped to the pocket floor
                     (m/spring 'joint 'neck :heel 'body :neck-joint :stiffness 25))
  :listens (list (m/pickup 'str (per-note (lambda (n) (from-bridge n (coil-from-bridge n))))
                           :height 3 :coil-hz 3000 :coil-q 1.5))
  :amp (m/amp :drive 1.5 :bias 0 :bloom 0 :bass 1.2 :mid 1 :treble 0.9 :power 0.8 :sag 0.1 :presence 0.2
              :cab 4000 :level 0.8 :low-cut 25 :cone-hz 50 :bass-hz 80))
