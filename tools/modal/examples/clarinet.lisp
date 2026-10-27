;; A clarinet: a cylindrical bore, closed at the reed and open at the bell
;; (odd harmonics), blown through a single reed (the normalised clarinet
;; model). It speaks above a blowing pressure of about 1/3 (blow), slowly at
;; first and faster the harder it is blown. Opening the tone hole two thirds
;; down the bore shortens the column and raises the pitch.
(import eseq.modal :as m)

(m/patch :name "modal-clarinet"
  :objects (list (m/tube 'bore :ends :closed-open :modes 20 :loss 3 :wall 1 :hf-loss 0.3))
  :connections (list (m/reed 'reed 'bore 0 :blow 0.7 :zeta 0.3)
                     (m/tone-hole 'hole 'bore 0.66 :open 0))
  :listens (list (m/listen 'bore 0.5 :gain 0.25)))
