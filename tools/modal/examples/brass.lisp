;; Brass: lips (a one-mode oscillator tuned near the note) buzzing into a
;; bore with a complete harmonic series (a conical approximation of a flared
;; horn). The pressure difference pushes the lips open; their opening lets the
;; flow in.
(import eseq.modal :as m)

(m/patch :name "modal-brass"
  :objects (list (m/oscillator 'lip :tune 1.05 :loss 60 :mass 0.5)
                 (m/tube 'bore :ends :cone :modes 16 :loss 2 :wall 1 :hf-loss 0.2))
  :connections (list (m/lips 'lips 'lip 'bore 0 :blow 0.8 :zeta 0.5 :beta 0.1 :rest 0.05 :coupling 1))
  :listens (list (m/listen 'bore 0.9 :gain 0.3)))
