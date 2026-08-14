;; ui/patch-learn.lisp — patch-editor direction-finding pane.
(module eseq.patch-learn)
(import eseq.browser)

(defstate %open false)

(def %param-status-color (status)
  (if (= status "learnable") :green
    (if (= status "frozen") :dim :red)))

(def %target-picker ()
  (eseq.browser/sample-browser-widget true SEQ.learn-target-path))

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

(def %configure-panel ()
  (v-stack :key "learn-configure" :width :fill :height :fill :gap 0.45
    (%target-picker)
    (%plan-list)
    (%config-control "EPOCHS" SEQ.learn-epochs 50 1000 0 "learn-epochs" :epochs)
    (%config-control "PITCH (HZ)" SEQ.learn-pitch-hz 10 20000 2 "learn-pitch" :pitch-hz)
    (%config-control "GATE (FRAMES)" SEQ.learn-gate-frames 1 192000 0 "learn-gate" :gate-frames)
    (button "Start training" :key "learn-start" :variant :primary :width :fill :height 1.45
      :on-click |x y r|
      (host-command "start-learn-job"
        (dict :epochs SEQ.learn-epochs :pitch-hz SEQ.learn-pitch-hz :gate-frames SEQ.learn-gate-frames))
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
  (h-stack :key (str "learn-live-param-" (get param :name)) :width :fill :gap 0.3 :align :center
    (label (get param :name) :font-size 8.5 :width 8 :color :white :bg :transparent)
    (%step-glyph (get param :step))
    (box :width 0 :flex 1)
    (label (fmt "{:.4}" (get param :value)) :font-size 8.5 :color :dim :bg :transparent)))

(def %loss-bars ()
  (box :key "learn-loss-curve" :width :fill :height 3.0 :background-color :bg :corner-radius 7 :padding 0.25
    (h-stack :width :fill :height :fill :gap 0.04 :align :end
      (each (range 0 (len SEQ.learn-losses)) |i|
        (let ((loss (nth SEQ.learn-losses i)))
          (box :width 0 :flex 1 :height (max 0.08 (min 2.5 (* loss 8))) :background-color :blue))))))

(def %training-panel ()
  (v-stack :key "learn-training" :width :fill :height :fill :gap 0.5
    (%target-picker)
    (label (if (= SEQ.learn-stage "basin-check") "BASIN CHECK" "TRAINING")
      :font-size 7.5 :color :blue :bg :transparent)
    (h-stack :width :fill :align :baseline
      (label (str "Epoch " SEQ.learn-current-epoch " / " SEQ.learn-total-epochs)
        :font-size 10 :color :white :bg :transparent)
      (box :width 0 :flex 1)
      (label (fmt "loss {:.6}" SEQ.learn-loss) :font-size 9 :color :dim :bg :transparent))
    (%loss-bars)
    (box :width :fill :height 0 :flex 1 :background-color :bg :corner-radius 7 :padding 0.35
      (scroll :width :fill :height :fill
        (v-stack :width :fill :gap 0.3
          (each (range 0 (len SEQ.learn-epoch-params)) |i|
            (%epoch-param-row (nth SEQ.learn-epoch-params i))))))
    (button "Stop" :key "learn-stop" :variant :secondary :width :fill :height 1.4
      :on-click |x y r| (host-command "stop-learn-job" (dict)) :color :white)))

(def %delta-row (delta)
  (h-stack :key (str "learn-delta-" (get delta :name)) :width :fill :gap 0.3 :align :baseline
    (label (get delta :name) :font-size 9 :width 8 :color :white :bg :transparent)
    (label (fmt "{:.4} → {:.4}" (get delta :from) (get delta :to))
      :font-size 8.5 :color :dim :bg :transparent)
    (box :width 0 :flex 1)
    (label (fmt "{:+.4}" (get delta :change)) :font-size 8.5
      :color (if (< (get delta :change) 0) :blue :green) :bg :transparent)))

(def %result-panel ()
  (v-stack :key "learn-result" :width :fill :height :fill :gap 0.5
    (%target-picker)
    (label (if (= SEQ.learn-basin-check "wrong_neighborhood")
        "Wrong neighborhood — seeded deltas are not trustworthy"
        (fmt "Improved {:.1}%" SEQ.learn-improvement-pct))
      :font-size 11 :color (if (= SEQ.learn-basin-check "wrong_neighborhood") :red :green) :bg :transparent)
    (label (fmt "distance {:.6}" SEQ.learn-abs-distance) :font-size 8.5 :color :dim :bg :transparent)
    (box :width :fill :height 0 :flex 1 :background-color :bg :corner-radius 7 :padding 0.35
      (scroll :width :fill :height :fill
        (v-stack :width :fill :gap 0.35
          (each (range 0 (len SEQ.learn-result-deltas)) |i|
            (%delta-row (nth SEQ.learn-result-deltas i))))))
    (h-stack :width :fill :gap 0.35
      (button "Target" :variant :secondary :flex 1 :height 1.35
        :on-click |x y r| (host-command "preview-sample" (dict :path SEQ.learn-target-path)) :color :white)
      (button "Learned" :variant :primary :flex 1 :height 1.35
        :on-click |x y r| (host-command "preview-sample" (dict :path SEQ.learn-final-wav)) :color :white))
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
        (if (= SEQ.learn-phase "training") (%training-panel)
          (if (= SEQ.learn-phase "result") (%result-panel)
            (%error-panel)))))))

(def panel ()
  (if %open
    (box :key "patch-learn-pane" :width 31 :height :fill :background-color :buffer-bg
      :corner-radius 9 :padding 0.55
      (v-stack :width :fill :height :fill :gap 0.45
        (h-stack :width :fill :align :baseline
          (label "PATCH LEARN" :font-size 11 :color :white :bg :transparent)
          (box :width 0 :flex 1)
          (button "×" :variant :ghost :width 2.2 :height 1.1
            :on-click |x y r| (set! %open false) :color :dim))
        (%body)))
    (button "Learn" :key "patch-learn-open" :variant :secondary :width 5.5 :height :fill
      :on-click |x y r| (set! %open true) :color :white)))
