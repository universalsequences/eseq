;; The modal guitar with fe-bottle's shape as its body: a finger plucks a steel
;; string whose bridge end is glued by a spring to the outside of the drilled
;; aluminium bottle (a finite-element solid), so the string rings through the
;; bottle's own modes. The body keeps its own pitch (no :root): only the string
;; follows the played note. The body's named :points are (x y z) in metres,
;; snapped to the nearest surface node; the spring pushes along that node's
;; normal. The bridge's and mic's "body point" knobs step through all four.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-fe-bottle-guitar"
  :objects (list (m/finger 'fin :depth 0.1)
                 (m/string 'str :made-of mat/steel :length 0.65 :radius-mm 0.5 :modes 60)
                 (m/solid 'body :made-of mat/aluminium :modes 40 :loss 1 :hf-loss 0.2 :mass 4
                          :shape (dict :difference (list (dict :cylinder (dict :axis :z :center (list -0.0127 0.0279 -0.0129) :height 0.5 :radius 0.1))
                                                         (dict :cylinder (dict :axis :z :center (list -0.0117 0.0277 0.0069) :height 0.46 :radius 0.0733))
                                                         (dict :cylinder (dict :axis :y :center (list -0.0527 0.0279 0.0353) :height 0.32 :radius 0.022))
                                                         (dict :cylinder (dict :axis :y :center (list 0.0273 0.0279 0.0353) :height 0.32 :radius 0.022))))
                          ;; named spots: the bridge and mics refer to these,
                          ;; and each point knob steps through all of them
                          :points (dict :side (list 0.0873 0.0279 -0.1)
                                        :shoulder (list 0.09 0.05 0.2)
                                        :lip (list -0.0127 0.1 0.237)
                                        :base (list -0.0127 0.0279 -0.263))))
  :connections (list (m/pluck 'pick 'fin 'str 0.8)
                     ;; the bridge: on the +x wall, below the holes
                     (m/spring 'bridge 'str 0.96 'body :side :stiffness 2))
  :listens (list (m/listen 'str 0.2 :gain 0.6)
                 (m/listen 'body :shoulder :gain 0.7))
  :level 1)
