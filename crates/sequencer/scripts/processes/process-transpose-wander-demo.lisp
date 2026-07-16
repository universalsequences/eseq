;; Process Channels demo: beat-quantized global transpose wander.
;;
;; Evaluate this file directly as an ESeqLisp buffer, or load it from project
;; scratch:
;;   (load "crates/sequencer/scripts/processes/process-transpose-wander-demo.lisp")
;;
;; Start the transport and put a few notes on a melodic track. This process walks
;; the scheduler global transpose once per beat, so opted-in tracks bend through a
;; deterministic up/down phrase. Track param `:global-transpose` can opt drums out.
;;
;; Useful live calls:
;;   (ps)
;;   (process-transpose-wander :step 2)
;;   (process-transpose-wander :range 12)
;;   (process-transpose-wander :track 1)
;;   (stop process-transpose-wander)
;;   (start process-transpose-wander)
;;   (seq-set-track-param :global-transpose false)
;;
;; Re-evaluating this buffer preserves named process state by cell name.

(def process-transpose-value
  (defchan process-transpose-value 0))

(def-process process-transpose-bounce
  :in ((step :float 0 12 :default 1)
       (range :float 0 24 :default 7)
       (track :int 0 63 :default 0))
  :out ((value :float))
  :state ((x 0)
          (dir 1)
          (tick 0)
          (marker 0))
  :every (beats 1)
  :run (do
         (set! tick (+ tick 1))
         (set! marker (+ marker 1))
         (set! x (+ x (* dir (in :step))))

         (if (> x (in :range))
           (do
             (set! x (in :range))
             (set! dir -1))
           nil)

         (if (< x (- 0 (in :range)))
           (do
             (set! x (- 0 (in :range)))
             (set! dir 1))
           nil)

         (transpose! x)
         (out :value x)
         (send :process-transpose-value x)

         ;; A small marker ping every four beats, routed to the selected track.
         (if (>= marker 4)
           (do
             (set! marker 0)
             (emit :track (in :track) :note 0 :vel 0.45 :duration 0.25))
           nil)))

(def process-transpose-wander
  (process-transpose-bounce :step 1 :range 7 :track 0))

(start process-transpose-wander)
(ps)
