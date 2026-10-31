;; A snare built the way Autechre's Modalys snare patch appears to be (from
;; a screen recording of it): one drum head, and two "snares" that are not
;; wires but strings, each touching the head at one point and rattling
;; against it (touch1, touch2: one-sided contacts), with a stick and two mics
;; on the head. Every point on the head is a dot on the panel's drum pad:
;; drag them. The strings alone slap like a sidestick; a bed of snare wires
;; under the head (m/snare-wires + m/snare-bed, as in snare.lisp) adds the
;; buzz.
;;
;;   1  stick   a light mallet on the head (felt)
;;   2  snare1  a heavy string pressed against the head at one point: the
;;              head slaps it, it hardly moves, so the head buzzes on it
;;   3  snare2  a second, of another material, at another point
;;   4  mic1    on the head, to the left
;;   5  mic2    on the head, to the right
;;      wires   eight wires resting on the head (buzz: Gap, Snap)
;;
;; From their screen: a 3 mm water skin, snares of water and spider silk,
;; heavy (Mass 100), each mic and each snare with its own left and right
;; level (mic1 and snare2 left, mic2 and snare1 right), a 110 Hz high-pass
;; on the output. Not copied: their loss numbers (Modalys' units are not
;; these: loss 1 here rings a head for seconds) and their note tracking.
;; Their snares follow the note, which played anywhere but low turns them
;; into a pitched string (a tinny piano); here they sit at a fixed low note
;; 25 (Note in off) and are damped hard, so they buzz rather than ring, and
;; the head is on the note (their Shift 12 puts it an octave up, a tiny
;; drum unless played low). They sit just under the head (Gap 0.02):
;; pressed into it (Gap below 0) they clamp it, a sidestick click with the
;; wires choked. The materials are what-ifs: a water skin is about as stiff for its
;; weight as nylon, spider silk three times that.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(def materials (list mat/water mat/spider-silk mat/mylar mat/nylon mat/steel))

(m/patch :name "modal-ae-snare"
  :objects (list (m/mallet 'stick :mass 0.02 :speed 1)
                 (m/membrane 'head :modes 64 :note 44 :shift 0 :loss 8 :bright 0.5
                             :made-of materials :thick-mm 3)
                 (m/string 'snare1 :made-of materials :note 25 :follow false :modes 48 :mass 100
                           :length 0.36 :radius-mm 0.3 :const-loss 15 :freq-loss 2 :mute 0)
                 (m/string 'snare2 :made-of (list mat/spider-silk mat/water mat/mylar mat/nylon mat/steel)
                           :note 25 :follow false :modes 48 :mass 100
                           :length 0.36 :radius-mm 0.3 :const-loss 15 :freq-loss 2 :mute 0)
                 (m/snare-wires 'wires :count 8 :modes 8 :pitch 1850 :loss 8 :mass 0.1))
  :connections (list (m/felt 'stick1 'stick 'head (list 0.51 2.94) :felt 0.01 :hardness 0.6)
                     (m/strike 'touch1 'head (list 0.345 4.42) 'snare1 0.37 :mic-gap true :offset 0.02 :stiffness 200)
                     (m/strike 'touch2 'head (list 0.453 1.46) 'snare2 0.41 :mic-gap true :offset 0.02 :stiffness 200)
                     (m/snare-bed 'buzz 'head 'wires :points 2 :iterations 3 :gap 0.005 :stiffness 150
                                  :drag 0 :snap 6))
  :listens (list (m/listen 'head (list 0.696 4.93) :left 1 :right 0)
                 (m/listen 'head (list 0.713 5.9) :left 0 :right 1)
                 (m/listen 'snare1 0.2 :gain 0.1 :left 0 :right 1)
                 (m/listen 'snare2 0.2 :gain 0.1 :left 1 :right 0))
  :out-filter (dict :type :highpass :cutoff 110 :q 0)
  :pad 'head
  :level 1)
