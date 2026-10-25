;; A gong that drives itself: a force at one point pushes with its own speed
;; at another (a feedback tap). The loop's small-signal gain is gain times
;; the sensitivity (above ~0.03 it grows), and :saturate caps the force at
;; gain, which sets where it settles: this lightly damped gong rings very
;; high, so the cap is tiny. Held notes bloom into a sustained roar at the
;; gong's strongest loop mode; the envelope opens the loop while the key is
;; down.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-feedback-gong"
  :objects (list (m/mallet 'beater :mass 0.05 :speed 1)
                 (m/circ-plate 'gong :made-of mat/bronze :radius 0.3 :thick-mm 2 :modes 64 :tune 3 :loss 0.6 :damp 0.1))
  :connections (list (m/felt 'hit 'beater 'gong (list 0.2 0) :felt 0.05)
                     (m/force 'loop 'gong (list 0.7 1.2) :gain 0.000004 :saturate true
                              :signal (list (m/envelope :attack-ms 400 :sustain 1 :release-ms 300)
                                            (m/tap-speed 'gong (list 0.45 2.6))
                                            (m/signal 11000))))
  :listens (list (m/listen 'gong (list 0.6 0.4))))
