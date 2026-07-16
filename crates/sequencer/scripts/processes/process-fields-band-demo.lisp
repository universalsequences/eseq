;; Slice 2 typed fields / band-model demo.
;;
;; Evaluate directly or load from project scratch:
;;   (load "crates/sequencer/scripts/processes/process-fields-band-demo.lisp")
;;
;; Prepare three melodic tracks with active steps and start transport:
;;   track 0 publishes a moving major-triad pitch field
;;   track 1 follows it fully
;;   track 2 leans halfway toward it
;;
;; `hear` sees the field published at the previous tick, never a same-tick
;; value. Disable/clear track 0 and both followers become inert automatically.
;; The publisher never names either listener; another track joins by attaching
;; `follow-harmony` with `:listen :harmony`.
;;
;; Useful live calls:
;;   (lane! fields-band-publisher-h :root 0 0 5 5 7 7 5 5)
;;   (fields-band-full-h :amount 0.75)
;;   (fields-band-half-h :amount 0.25)
;;   (processes :track 0) ; no publisher => listeners are inert

(seq-register-script-source-tab "Fields Band Demo")

(def-process fields-band-publisher
  :doc "Suggest a major-triad pitch field rooted by a sequenceable lane."
  :in ((root :float -24 24 :default 0 :lane true)
       (weight :float 0 1 :default 1 :lane true))
  :run (suggest :harmony
         (pitch-field
           (list (in :root) (+ (in :root) 4) (+ (in :root) 7))
           :root (in :root)
           :weight (in :weight))))

(def fields-band-publisher-h
  (fields-band-publisher
    :root (lane 0 0 5 5 7 7 5 5)
    :weight (lane 1 1 1 1 0.8 0.8 1 1)))

(def fields-band-full-h
  (follow-harmony :listen :harmony :amount (lane 1) :grace 0))

(def fields-band-half-h
  (follow-harmony :listen :harmony :amount (lane 0.5) :grace 0))

(processes :track 0 fields-band-publisher-h)
(processes :track 1 fields-band-full-h)
(processes :track 2 fields-band-half-h)
