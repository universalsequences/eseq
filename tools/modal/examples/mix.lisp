;; A mix hybrid: one pluck shared between two strings a fifth apart; morph
;; slides the pluck (and the listening) from the lower string to the upper,
;; both heard in between.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-mix"
  :objects (list (m/finger 'fin :depth 0.12)
                 (m/string 'low :made-of mat/steel :modes 48)
                 (m/string 'high :made-of mat/steel :modes 48 :tune 1.5)
                 (m/mix-hybrid 'pair 'low 'high :morph 0.3))
  :connections (list (m/pluck 'pick 'fin 'pair (list 0.2 0.2)))
  :listens (list (m/listen 'pair (list 0.8 0.8))))
