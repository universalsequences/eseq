;; A sitar-like buzz: a plucked steel string ringing against a rigid stop just
;; under it near the bridge (the jawari), so each swing grazes the stop and
;; spreads energy up the spectrum; bridged onto a gourd-ish plate.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-sitar"
  :objects (list (m/finger 'fin :depth 0.15)
                 (m/string 'str :made-of mat/steel :length 0.9 :radius-mm 0.2 :modes 80 :const-loss 0.2 :freq-loss 0.2)
                 (m/plate 'body :made-of mat/spruce :mass 15))
  :connections (list (m/pluck 'pick 'fin 'str 0.25)
                     (m/stop 'jaw 'str 0.03 :height -0.0004 :stiffness 400)
                     (m/spring 'bridge 'str 0.99 'body (list 0.55 0.4) :stiffness 0.5))
  :listens (list (m/listen 'str 0.15 :gain 0.6) (m/listen 'body (list 0.3 0.65) :gain 0.5)))
