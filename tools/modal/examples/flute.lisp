;; A flute: a bore open at both ends (every harmonic), driven by an air jet
;; across the embouchure near one end: the breath plus the air's velocity at
;; the mouth, delayed by the jet's travel (ratio, in periods of the note) and
;; saturated, with a little turbulence. It speaks on the note's harmonic
;; series; the jet delay picks the register, and as tuned here it favours an
;; overblown register (the 3rd harmonic): speaking on the fundamental is
;; open work (eseq-k0m2.33).
(import eseq.modal :as m)

(m/patch :name "modal-flute"
  :objects (list (m/tube 'bore :ends :open-open :modes 16 :loss 2 :wall 3 :hf-loss 3))
  :connections (list (m/jet 'jet 'bore 0.05 :blow 0.5 :ratio 1 :feedback 10 :noise 0.05 :gain 0.3))
  :listens (list (m/listen 'bore 0.25 :gain 0.3)))
