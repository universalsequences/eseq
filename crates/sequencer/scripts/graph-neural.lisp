;; Canonical graph-mode neural sequencer.
;;
;; Per-pattern/UI edits should be stored with graph-node, graph-param, and graph-edge
;; overrides rather than by mutating this source definition.

(def-sequencer "neural"
  :shape (grid 8 8)
  :energy-decay 0.994
  :reset-every (bars 4)
  :seed-on-reset 0
  :max-poly 2
  :max-poly-selection :deterministic

  (def-node nrn
    :resolution :16
    :delay 1
    :quantize :off
    :route 0
    :seed-from :route
    :reduce :sum
    :params ((threshold :float 0 4 :default 1)
             (transpose :int -48 48 :default 0)
             (dampening :float 0 1 :default 0)
             (recovery :float 0 1 :default 0.98))
    :state ((energy :leak (per-step :energy-decay)))
    :update (if (>= (node-state self :energy)
                    (node-param self :threshold))
              (do
                (dampen-incoming self (node-param self :dampening))
                true)
              (do
                (recover-incoming self (node-param self :recovery))
                false)))

  (edges
    :from nrn
    :to nrn
    :topology (all-to-all)
    :gather (- (edge :weight) (edge :dampening))
    :params ((weight :float -1 1 :default 0)
             (dampening :float 0 1 :default 0))))
