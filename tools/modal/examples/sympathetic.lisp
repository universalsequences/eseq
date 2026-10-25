;; One plucked string and two undamped strings tuned a 5th and an octave up,
;; all bridged onto one plate: the plate passes the plucked string's energy
;; to whichever sympathetic partials line up (sitar / sarangi drones).
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-sympathetic"
  :objects (list (m/finger 'fin :depth 0.1)
                 (m/string 'main :made-of mat/steel :modes 48)
                 (m/string 'fifth :made-of mat/steel :modes 48 :tune 1.5 :mute 0 :const-loss 0.1 :freq-loss 0.2)
                 (m/string 'octave :made-of mat/steel :modes 48 :tune 2 :mute 0 :const-loss 0.1 :freq-loss 0.2)
                 (m/plate 'body :made-of mat/spruce :mass 10))
  :connections (list (m/pluck 'pick 'fin 'main 0.8)
                     (m/spring 'b1 'main 0.96 'body (list 0.5 0.45) :stiffness 2)
                     (m/spring 'b2 'fifth 0.96 'body (list 0.55 0.5) :stiffness 2)
                     (m/spring 'b3 'octave 0.96 'body (list 0.6 0.55) :stiffness 2))
  :listens (list (m/listen 'main 0.2 :gain 0.5)
                 (m/listen 'fifth 0.2 :gain 1)
                 (m/listen 'octave 0.2 :gain 1)
                 (m/listen 'body (list 0.3 0.65) :gain 0.5))
  :level 2)
