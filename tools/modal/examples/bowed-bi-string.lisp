;; A bowed string in two directions (Modalys' bi-string): the bow's friction
;; drives the first direction while its hair presses the second down, and
;; the friction pressure follows that contact's force, so the string's
;; up-and-down motion modulates the bowing (the second direction is detuned a
;; hair by split, so the two beat). Bridged onto a body from both directions.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-bowed-bi-string"
  :objects (list (m/string 'str :made-of mat/nylon :length 0.69 :radius-mm 0.7 :modes 60 :planes 2
                                :const-loss 0.5 :freq-loss 0.4 :mute 0)
                 (m/plate 'body :made-of mat/spruce :width 0.75 :height 0.45 :thick-mm 4 :mass 40))
  :connections (list (m/bow 'bow 'str 0.12 :hair 0.002)
                     (m/spring 'bridge 'str 0.99 'body (list 0.55 0.4) :stiffness 0.3)
                     (m/spring 'bridge_v (m/plane 'str 1) 0.99 'body (list 0.55 0.4) :stiffness 0.3))
  :listens (list (m/listen 'str 0.85 :gain 0.5) (m/listen (m/plane 'str 1) 0.85 :gain 0.3)
                 (m/listen 'body (list 0.3 0.65) :gain 0.7)))
