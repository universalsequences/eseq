;; ui/patch-learn.lisp — patch-editor direction-finding pane.
(module eseq.patch-learn)
(import eseq.browser)
(import eseq.seq-layout)
(import eseq.seq-step-tabs)

(def %close ()
  (if (= eseq.seq-step-tabs/seq-patcher-buffer "")
    (status "No instrument patcher buffer is active")
    (do
      (host-command "close-learn-patch" (dict))
      (eseq.seq-layout/apply-instrument-patcher-layout eseq.seq-step-tabs/seq-patcher-buffer))))

(def %param-status-color (status)
  (if (= status "learnable") :green
    (if (= status "frozen") :dim :red)))

(def %target-picker ()
  (eseq.browser/sample-browser-widget
    true SEQ.learn-target-path SEQ.learn-target-name))

(def %plan-row (param)
  (v-stack :key (str "learn-plan-" (get param :name)) :width :fill :gap 0.05
    (h-stack :width :fill :gap 0.4 :align :baseline
      (label (get param :name) :font-size 9.5 :color :white :bg :transparent)
      (box :width 0 :flex 1)
      (label (get param :status)
        :font-size 7.5 :color (%param-status-color (get param :status)) :bg :transparent))
    (if (= (get param :reason) "")
      (box :height 0)
      (label (get param :reason) :font-size 7.5 :color :dim :bg :transparent))))

(def %plan-list ()
  (box :key "learn-plan-list" :width :fill :height 8.5 :background-color :bg :corner-radius 7 :padding 0.35
    (scroll :width :fill :height :fill
      (v-stack :width :fill :gap 0.3
        (each (range 0 (len SEQ.learn-plan-params)) |i|
          (%plan-row (nth SEQ.learn-plan-params i)))))))

(def %config-control (label-text value min-value max-value decimals key-name field)
  (v-stack :width :fill :gap 0.15
    (label label-text :font-size 8 :color :dim :bg :transparent)
    (number-picker :key key-name :width :fill :height 1.25
      :value value :min min-value :max max-value :decimals decimals
      :on-change (lambda (v)
        (host-command "configure-learn" (dict field v))))))

(def %dropdown-control (label-text value options key-name field)
  (v-stack :width :fill :gap 0.15
    (label label-text :font-size 8 :color :dim :bg :transparent)
    (dropdown :key key-name :width :fill :height 1.25
      :value value :options options
      :on-change (lambda (v)
        (host-command "configure-learn" (dict field v))))))

(def cma-config ()
  (v-stack :width :fill :gap 0.45
    (%config-control "GENERATIONS" SEQ.learn-cma-generations 1 1000 0
      "learn-cma-generations" :cma-generations)
    (%config-control "CANDIDATES / GENERATION (0 = AUTO)" SEQ.learn-cma-population 0 4096 0
      "learn-cma-population" :cma-population)
    (label
      (str (if (= SEQ.learn-cma-population 0) "Estimated up to " "Up to ")
        (* SEQ.learn-cma-generations
          (if (= SEQ.learn-cma-population 0) 32 SEQ.learn-cma-population))
        " candidate evaluations"
        (if (= SEQ.learn-cma-population 0) " (trainer resolves auto population)" ""))
      :font-size 7.5 :color :dim :bg :transparent)
    (%config-control "INITIAL SPREAD (SIGMA)" SEQ.learn-cma-sigma 0.01 10 3
      "learn-cma-sigma" :cma-sigma)
    (%config-control "RANDOM SEED" SEQ.learn-cma-seed 0 4294967295 0
      "learn-cma-seed" :cma-seed)
    (%config-control "FORWARD BATCH (0 = AUTO)" SEQ.learn-cma-forward-batch 0 4096 0
      "learn-cma-forward-batch" :cma-forward-batch)
    (if (= SEQ.learn-method "Evolutionary search only")
      (box :height 0)
      (v-stack :width :fill :gap 0.45
        (%config-control "LOCAL FALLBACK EPOCHS (0 = OFF)" SEQ.learn-local-epochs 0 2000 0
          "learn-local-epochs" :local-epochs)
        (%config-control "SHORTLIST CANDIDATES (0 = BEST ONLY)" SEQ.learn-cma-continue 0 4096 0
          "learn-cma-continue" :cma-continue)
        (%config-control "SHORTLIST ADAM EPOCHS (0 = OFF)" SEQ.learn-cma-refine-epochs 0 2000 0
          "learn-cma-refine-epochs" :cma-refine-epochs)
        (%dropdown-control "SHORTLIST EXECUTION" SEQ.learn-cma-refine-mode
          (list "Batched" "Scalar" "Auto") "learn-cma-refine-mode" :cma-refine-mode)
        (%config-control "WINNER TRAINING EPOCHS (0 = OFF)" SEQ.learn-cma-final-epochs 0 2000 0
          "learn-cma-final-epochs" :cma-final-epochs)))))

(def %start-label ()
  (if (= SEQ.learn-method "Local fit + basin check") "Start local training"
    (if (= SEQ.learn-method "Evolutionary search only") "Start evolutionary search"
      "Search, polish, and train winner")))

(def %start-payload ()
  (dict
    :method SEQ.learn-method
    :epochs SEQ.learn-epochs
    :cma-generations SEQ.learn-cma-generations
    :cma-population SEQ.learn-cma-population
    :cma-sigma SEQ.learn-cma-sigma
    :cma-seed SEQ.learn-cma-seed
    :cma-forward-batch SEQ.learn-cma-forward-batch
    :local-epochs SEQ.learn-local-epochs
    :cma-continue SEQ.learn-cma-continue
    :cma-refine-epochs SEQ.learn-cma-refine-epochs
    :cma-refine-mode SEQ.learn-cma-refine-mode
    :cma-final-epochs SEQ.learn-cma-final-epochs
    :pitch-hz SEQ.learn-pitch-hz
    :gate-frames SEQ.learn-gate-frames))

(def %configure-panel ()
  (v-stack :key "learn-configure" :width :fill :height :fill :gap 0.45
    (%target-picker)
    (%plan-list)
    (box :width :fill :height 0 :flex 1
      (scroll :width :fill :height :fill
        (v-stack :width :fill :gap 0.45
          (%dropdown-control "TRAINING METHOD" SEQ.learn-method
            (list "Local fit + basin check" "Evolutionary search only" "Evolutionary search + training")
            "learn-method" :method)
          (if (= SEQ.learn-method "Local fit + basin check")
            (%config-control "ADAM EPOCHS" SEQ.learn-epochs 1 2000 0 "learn-epochs" :epochs)
            (cma-config))
          (%config-control "PITCH (HZ)" SEQ.learn-pitch-hz 10 20000 2 "learn-pitch" :pitch-hz)
          (%config-control "GATE (FRAMES)" SEQ.learn-gate-frames 1 192000 0 "learn-gate" :gate-frames))))
    (button (%start-label) :key "learn-start" :variant :primary :width :fill :height 1.45
      :on-click |x y r| (host-command "start-learn-job" (%start-payload))
      :color :white)))

(def %step-glyph (step)
  (let ((magnitude (min 1 (abs step))))
    (box :width 4.5 :height 0.55 :background-color :bg :corner-radius 4
      (h-stack :width :fill :height :fill :align :center
        (box :width 2.1 :height 0.12 :background-color :transparent :h-align :end
          (box :width (* 2.1 magnitude) :height 0.12
            :background-color (if (< step 0) :blue :transparent)))
        (box :width 0.15 :height 0.5 :background-color :dim)
        (box :width 2.1 :height 0.12 :background-color :transparent :h-align :start
          (box :width (* 2.1 magnitude) :height 0.12
            :background-color (if (> step 0) :green :transparent)))))))

(def %epoch-param-row (param)
  (v-stack :key (str "learn-live-param-" (get param :name)) :width :fill :gap 0.08
    (h-stack :width :fill :gap 0.3 :align :center
      (label (get param :name) :font-size 8.5 :width 8 :color :white :bg :transparent)
      (%step-glyph (get param :step))
      (box :width 0 :flex 1)
      (label (fmt "{:+.4}" (get param :change)) :font-size 8.5
        :color (if (< (get param :change) 0) :blue :green) :bg :transparent))
    (label (fmt "{:.4} → {:.4}" (get param :from) (get param :value))
      :font-size 7.5 :color :dim :bg :transparent)))

(def %loss-graph (losses total-epochs)
  (box :key "learn-loss-curve" :width :fill :height 3.0 :background-color :bg :corner-radius 7 :padding 0.25
    (linegraph
      :width :fill :height :fill
      :values losses
      :total-points total-epochs
      :min 0
      :y-axis true
      :line-color :blue
      :area true)))

(def %stage-label (stage)
  (if (or (= stage "") (= stage "starting")) "STARTING"
    (if (= stage "cma-es") "EVOLUTIONARY SEARCH"
    (if (= stage "cma-refine-batched") "BATCHED SHORTLIST POLISH"
      (if (= stage "cma-final") "WINNER TRAINING"
        (if (= stage "basin-check") "BASIN CHECK"
          (if (= stage "train")
            (if (= SEQ.learn-method "Local fit + basin check") "LOCAL TRAINING" "LOCAL FALLBACK")
            "SHORTLIST POLISH")))))))

(def %stage-has-epochs (stage)
  (and (not (= stage "")) (not (= stage "starting"))
    (not (= stage "cma-es")) (not (= stage "cma-refine-batched"))))

(def %optimization-loss-row (stage index loss)
  (h-stack :key (str "learn-optimization-loss-" index) :width :fill :align :baseline
    (label (if (= stage "cma-es") (str "Candidate " (+ index 1)) "Mean batch loss")
      :font-size 8 :color :dim :bg :transparent)
    (box :width 0 :flex 1)
    (label (fmt "{:.6}" loss) :font-size 8.5 :color :white :bg :transparent)))

(def %non-epoch-stage-progress (stage current total optimization-losses losses)
  (box :key "learn-stage-progress" :width :fill :height 6.5
    :background-color :bg :corner-radius 7 :padding 0.55
    (v-stack :width :fill :height :fill :gap 0.35
      (label (if (or (= stage "") (= stage "starting")) "Preparing training job…"
          (if (= stage "cma-es")
            (str "Generation " current " / " total)
            (str "Epoch " current " / " total)))
        :font-size 10 :color :white :bg :transparent)
      (if (= (len optimization-losses) 0)
        (label "Waiting for the first optimizer update…"
          :font-size 7.5 :color :dim :bg :transparent)
        (v-stack :width :fill :gap 0.15
          (each (range 0 (len optimization-losses)) |i|
            (%optimization-loss-row stage i (nth optimization-losses i)))))
      (if (and (= stage "cma-refine-batched") (> (len losses) 1))
        (%loss-graph losses total)
        (box :height 0)))))

(def training-panel (target-path target-name stage current-epoch total-epochs loss losses optimization-losses epoch-params)
  (v-stack :key "learn-training" :width :fill :height :fill :gap 0.5
    (eseq.browser/sample-browser-widget true target-path target-name)
    (label (%stage-label stage) :font-size 7.5 :color :blue :bg :transparent)
    (if (%stage-has-epochs stage)
      (v-stack :width :fill :gap 0.5
        (h-stack :width :fill :align :baseline
          (label (str "Epoch " current-epoch " / " total-epochs)
            :font-size 10 :color :white :bg :transparent)
          (box :width 0 :flex 1)
          (label (fmt "loss {:.6}" loss) :font-size 9 :color :dim :bg :transparent))
        (%loss-graph losses total-epochs))
      (%non-epoch-stage-progress stage current-epoch total-epochs optimization-losses losses))
    (box :width :fill :height 0 :flex 1 :background-color :bg :corner-radius 7 :padding 0.35
      (scroll :width :fill :height :fill
        (v-stack :width :fill :gap 0.3
          (each (range 0 (len epoch-params)) |i|
            (%epoch-param-row (nth epoch-params i))))))
    (button "Stop" :key "learn-stop" :variant :secondary :width :fill :height 1.4
      :on-click |x y r| (host-command "stop-learn-job" (dict)) :color :white)))

(def %delta-row (delta)
  (h-stack :key (str "learn-delta-" (get delta :name)) :width :fill :gap 0.3 :align :baseline
    (label (get delta :name) :font-size 9 :width 8 :color :white :bg :transparent)
    (label (fmt "{:.4} → {:.4}" (get delta :from) (get delta :to))
      :font-size 8.5 :color :dim :bg :transparent)
    (box :width 0 :flex 1)
    (label (str (if (< (get delta :change) 0) "−" "+")
                (fmt "{:.4}" (abs (get delta :change)))) :font-size 8.5
      :color (if (< (get delta :change) 0) :blue :green) :bg :transparent)))

(def result-panel (target-path target-name improvement-pct abs-distance basin-check deltas seeded-wav final-wav applied)
  (v-stack :key "learn-result" :width :fill :height :fill :gap 0.5
    (eseq.browser/sample-browser-widget true target-path target-name)
    (label (if (= basin-check "wrong_neighborhood")
        "Wrong neighborhood — seeded deltas are not trustworthy"
        (fmt "Improved {:.1}%" improvement-pct))
      :font-size 11 :color (if (= basin-check "wrong_neighborhood") :red :green) :bg :transparent)
    (label (fmt "distance {:.6}" abs-distance) :font-size 8.5 :color :dim :bg :transparent)
    (label "PARAMETER TRAVEL" :font-size 7.5 :color :dim :bg :transparent)
    (box :key "learn-result-list" :width :fill :height 15 :background-color :bg :corner-radius 7 :padding 0.35
      (scroll :width :fill :height :fill
        (v-stack :width :fill :gap 0.35
          (each (range 0 (len deltas)) |i|
            (%delta-row (nth deltas i))))))
    (h-stack :width :fill :gap 0.35
      (button "Target" :variant :secondary :flex 1 :height 1.35
        :on-click |x y r| (host-command "preview-sample" (dict :path target-path)) :color :white)
      (button "Seeded" :variant :secondary :flex 1 :height 1.35
        :on-click |x y r| (host-command "preview-sample" (dict :path seeded-wav)) :color :white)
      (button "Learned" :variant :primary :flex 1 :height 1.35
        :on-click |x y r| (host-command "preview-sample" (dict :path final-wav)) :color :white))
    (label "The live instrument is previewing these learned values. Back or close restores the seed."
      :width :fill :height 1.7 :wrap true :font-size 7 :color :dim :bg :transparent)
    (if applied
      (button "Applied — undo available" :variant :secondary :width :fill :height 1.35
        :on-click |x y r| nil :color :green)
      (button "Apply to instrument" :variant :primary :width :fill :height 1.35
        :on-click |x y r| (host-command "apply-learn-result" (dict)) :color :white))
    (button "Back to configure" :variant :ghost :width :fill :height 1.2
      :on-click |x y r| (host-command "replan-learn-job" (dict)) :color :dim)))

(def %error-panel ()
  (v-stack :key "learn-error" :width :fill :height :fill :gap 0.6
    (%target-picker)
    (label "Learning stopped" :font-size 11 :color :red :bg :transparent)
    (label SEQ.learn-error :font-size 9 :color :white :bg :transparent)
    (label "If pitch detection failed, enter the target pitch before retrying."
      :font-size 7.5 :color :dim :bg :transparent)
    (%config-control "PITCH (HZ)" SEQ.learn-pitch-hz 10 20000 2 "learn-error-pitch" :pitch-hz)
    (%config-control "GATE (FRAMES)" SEQ.learn-gate-frames 1 192000 0 "learn-error-gate" :gate-frames)
    (button "Re-run plan" :variant :secondary :width :fill :height 1.35
      :on-click |x y r|
      (host-command "replan-learn-job"
        (dict :pitch-hz SEQ.learn-pitch-hz :gate-frames SEQ.learn-gate-frames))
      :color :white)))

(def %body ()
  (if (= SEQ.learn-phase "pick") (%target-picker)
    (if (= SEQ.learn-phase "planning")
      (v-stack :width :fill :gap 0.5
        (%target-picker)
        (label "Analyzing learnable parameters…" :font-size 10 :color :dim :bg :transparent))
      (if (= SEQ.learn-phase "configure") (%configure-panel)
        (if (= SEQ.learn-phase "training")
          (training-panel
            SEQ.learn-target-path SEQ.learn-target-name
            SEQ.learn-stage SEQ.learn-current-epoch SEQ.learn-total-epochs
            SEQ.learn-loss SEQ.learn-losses SEQ.learn-optimization-losses
            SEQ.learn-epoch-params)
          (if (= SEQ.learn-phase "result")
            (result-panel
              SEQ.learn-target-path SEQ.learn-target-name
              SEQ.learn-improvement-pct SEQ.learn-abs-distance
              SEQ.learn-basin-check SEQ.learn-result-deltas
              SEQ.learn-seeded-wav SEQ.learn-final-wav
              SEQ.learn-applied)
            (%error-panel)))))))

(def panel ()
  (box :key "patch-learn-pane" :width :fill :height :fill :background-color :buffer-bg
    :padding 0.55
    (v-stack :width :fill :height :fill :gap 0.45
      (h-stack :width :fill :align :baseline
        (label "PATCH LEARN" :font-size 11 :color :white :bg :transparent)
        (box :width 0 :flex 1)
        (button "×" :variant :ghost :width 2.2 :height 1.1
          :on-click |x y r| (%close) :color :dim))
      (box :key "patch-learn-body" :width :fill :height 0 :flex 1
        (%body)))))
