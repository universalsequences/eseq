;; Process-driven homeostat for graph-neural-variable-reset-demo.lisp.
;;
;; Load the graph sequencer first, route it to track 0, then load this file:
;;
;;   (load "crates/sequencer/scripts/sequencers/graph-neural-variable-reset-demo.lisp")
;;   (script-init-fn)
;;   (load "crates/sequencer/scripts/processes/graph-homeostat-demo.lisp")
;;
;; System 3 measures the audible track once per bar and regulates density plus
;; pitch spread through ephemeral graph deltas. System 4 watches System 3's
;; published strain and changes operating regime when regulation stays saturated.
;; Stop/restart clears every delta; use the graph panel's commit button to promote
;; a useful drift into authored overrides.

(seq-register-script-source-tab "Graph Homeostat")

(def gh-name "neural-variable-reset-demo")
(def gh-nodes 8)

(def gh-strain
  (defchan gh-strain 0))

(def gh-recent-pitches (track-index count)
  (map
    (lambda (i)
      (read (track track-index :transpose :trigs-ago i)))
    (range 0 count)))

(def gh-spread (values)
  (if (> (len values) 0)
    (- (apply max values) (apply min values))
    0))

(def-process graph-homeostat
  :doc "System 3: keep graph density and pitch spread inside an authored viability envelope."
  :in ((track :track :default 0)
       (density-lo :int 0 32 :default 3)
       (density-hi :int 0 32 :default 9)
       (spread-min :int 0 48 :default 12)
       (gain :float 0 1 :default 0.25 :lane true))
  :state ((strain 0))
  :every (bars 1)
  :run
  (let ((fires (read (track (in :track) :fire-count :window (bars 1))))
        (spread (gh-spread (gh-recent-pitches (in :track) 8))))
    (do
      (let ((error (if (< fires (in :density-lo))
                     (- (in :density-lo) fires)
                     (if (> fires (in :density-hi))
                       (- (in :density-hi) fires)
                       0))))
        (if (= error 0)
          (set! strain (* strain 0.5))
          (do
            (set! strain (+ strain (abs error)))
            (for-each
              (lambda (node)
                (do
                  (graph-nudge-edge! gh-name
                    :from node
                    :to (mod (+ node 1) gh-nodes)
                    :weight (* error 0.02 (in :gain)))
                  (graph-nudge-node! gh-name
                    node :delay (* error -0.15 (in :gain)))))
              (range 0 gh-nodes)))))
      (if (< spread (in :spread-min))
        (for-each
          (lambda (node)
            (graph-nudge-param! gh-name node :transpose
              (* (if (= (mod node 2) 0) 1 -1)
                 (- (in :spread-min) spread)
                 0.1
                 (in :gain))))
          (range 0 gh-nodes))
        nil)
      (send :gh-strain strain))))

(def-process graph-restructurer
  :doc "System 4: rotate cross-ring shortcuts after sustained homeostat strain."
  :in ((patience :int 1 8 :default 2)
       (kick :float 0 1 :default 0.6))
  :state ((hot-windows 0)
          (regime 0))
  :every (bars 4)
  :run
  (let ((strain (read :channel :gh-strain)))
    (if (and strain (> strain 6))
      (do
        (set! hot-windows (+ hot-windows 1))
        (if (>= hot-windows (in :patience))
          (do
            (set! regime (+ regime 1))
            (set! hot-windows 0)
            (graph-clear-deltas! gh-name)
            (for-each
              (lambda (node)
                (do
                  (graph-nudge-edge! gh-name
                    :from node
                    :to (mod (+ node 3 regime) gh-nodes)
                    :weight (* 0.3 (in :kick)))
                  (graph-nudge-param! gh-name node :threshold
                    (* -0.1 (in :kick)))))
              (range 0 gh-nodes)))
          nil))
      (set! hot-windows (max 0 (- hot-windows 1))))))

(def gh-system3
  (graph-homeostat :track 0 :gain 0.25))
(def gh-system4
  (graph-restructurer))

(start gh-system3)
(start gh-system4)
(ps)
