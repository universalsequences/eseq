;; A bowed string in two directions (Modalys' bi-string): the bow's friction
;; drives the first direction while its hair presses the second down, and
;; the friction pressure follows that contact's force, so the string's
;; up-and-down motion modulates the bowing (the second direction is detuned a
;; hair by split, so the two beat). Bridged onto a body from both directions.
;; The bridge, gauge and body are the cello's (examples/cello.lisp says why).
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-bowed-bi-string"
  :objects (list (m/string 'str :made-of mat/nylon :length 0.69 :radius-mm (m/by-note 65 0.15 523 1.2) :modes 60 :planes 2
                                :const-loss 0.5 :freq-loss 1.2 :mute 0)
                 (m/plate 'body :made-of mat/spruce :width 0.5 :height 0.35 :thick-mm 5 :mass 40 :loss 30 :damp 5))
  :connections (list (m/bow 'bow 'str 0.12 :pressure 0.3 :hair 0.002)
                     (m/spring 'bridge 'str 0.999 'body (list 0.55 0.4) :stiffness 0.3)
                     (m/spring 'bridge_v (m/plane 'str 1) 0.999 'body (list 0.55 0.4) :stiffness 0.3))
  :listens (list (m/listen 'str 0.97 :gain 0.5) (m/listen (m/plane 'str 1) 0.97 :gain 0.3)
                 (m/listen 'body (list 0.3 0.65) :gain 0.3)))
