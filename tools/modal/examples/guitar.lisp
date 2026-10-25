;; The modal-guitar patch, as data: finger -pluck-> string -bridge-> plate.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-guitar-gen"
  :objects (list (m/finger 'fin :depth 0.1)
                 (m/string 'str :made-of mat/steel :length 0.65 :radius-mm 0.5 :modes 60)
                 (m/plate 'body :made-of (list mat/spruce mat/maple mat/rosewood)))
  :connections (list (m/pluck 'pick 'fin 'str 0.8)
                     (m/spring 'bridge 'str 0.96 'body (list 0.55 0.4) :stiffness 2))
  :listens (list (m/listen 'str 0.2 :gain 0.6)
                 (m/listen 'body (list 0.3 0.65) :gain 0.7))
  :level 1)
