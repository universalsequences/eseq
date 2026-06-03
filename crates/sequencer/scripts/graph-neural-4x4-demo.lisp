;; Compact graph-mode neural sequencer for hands-on testing.
;;
;; Seed it from track 0. Node 0 is the only external seed point; the graph then
;; routes accepted firings across the first five tracks through per-pattern
;; graph-node overrides at the bottom of this file.

(def-sequencer "neural-4x4-demo"
  :shape (grid 4 4)
  :energy-decay 0.992
  :reset-every (bars 4)
  :seed-on-reset 0
  :max-poly 3
  :max-poly-selection :propagation

  (def-node nrn
    :resolution :16
    :delay 1
    :quantize :16
    :route 0
    :seed-from ()
    :reduce :sum
    :params ((threshold :float 0 4 :default 0.55)
             (transpose :int -48 48 :default 0)
             (dampening :float 0 1 :default 0.14)
             (recovery :float 0 1 :default 0.94))
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
    :params ((weight :float -1 1 :default 0.62)
             (dampening :float 0 1 :default 0))))

;; Demo plocks. These are sparse current-pattern graph overrides, not edits to the
;; published graph definition above.

(graph-node "neural-4x4-demo" 0  :delay 1 :route 0 :seed-from 0)
(graph-node "neural-4x4-demo" 1  :delay 1 :route 1)
(graph-node "neural-4x4-demo" 2  :delay 2 :route 2)
(graph-node "neural-4x4-demo" 3  :delay 1 :route 3)

(graph-node "neural-4x4-demo" 4  :delay 2 :route 1)
(graph-node "neural-4x4-demo" 5  :delay 1 :route 2)
(graph-node "neural-4x4-demo" 6  :delay 3 :route 3)
(graph-node "neural-4x4-demo" 7  :delay 2 :route 4)

(graph-node "neural-4x4-demo" 8  :delay 1 :route 2)
(graph-node "neural-4x4-demo" 9  :delay 2 :route 3)
(graph-node "neural-4x4-demo" 10 :delay 1 :route 4)
(graph-node "neural-4x4-demo" 11 :delay 3 :route 0)

(graph-node "neural-4x4-demo" 12 :delay 2 :route 3)
(graph-node "neural-4x4-demo" 13 :delay 1 :route 4)
(graph-node "neural-4x4-demo" 14 :delay 2 :route 0)
(graph-node "neural-4x4-demo" 15 :delay 1 :route 1)
