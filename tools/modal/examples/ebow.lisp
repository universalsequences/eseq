;; An e-bow: a force at the note's own frequency, faded in by an envelope,
;; keeps a steel string ringing for as long as the key is held (the force
;; drives the fundamental resonantly), bridged onto a spruce body.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-ebow"
  :objects (list (m/string 'str :made-of mat/steel :length 0.65 :radius-mm 0.4 :mute 8)
                 (m/plate 'body :made-of mat/spruce :mass 20))
  :connections (list (m/force 'ebow 'str 0.3 :gain 0.03
                              :signal (list (m/envelope :attack-ms 250 :sustain 1 :release-ms 150)
                                            (m/sine :ratio 1)))
                     (m/spring 'bridge 'str 0.99 'body (list 0.55 0.4)))
  :listens (list (m/listen 'str 0.2 :gain 0.7) (m/listen 'body (list 0.3 0.65) :gain 0.5)))
