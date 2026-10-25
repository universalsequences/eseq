;; finger -pluck-> string -bridge-> membrane: a banjo. The bridge stands on a
;; drum head instead of a wooden plate; the head follows the note an octave
;; down (tune 0.5) so its ring stays under the string.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-banjo"
  :objects (list (m/finger 'fin :hz 600 :depth 0.1 :stroke-ms 15)
                 (m/string 'str :made-of mat/steel :length 0.66 :radius-mm 0.25 :modes 60 :const-loss 0.8 :freq-loss 0.8)
                 (m/membrane 'head :loss 10 :bright 2 :tune 0.5 :modes 80))
  :connections (list (m/pluck 'pick 'fin 'str 0.9)
                     (m/spring 'bridge 'str 0.95 'head (list 0.5 0) :stiffness 4))
  :listens (list (m/listen 'str 0.2 :gain 0.4)
                 (m/listen 'head (list 0.6 1) :gain 1))
  :level 2.5)
