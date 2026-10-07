;; ui/patch-learn.lisp — patch-editor direction-finding pane.
;;
;; Everything it shows is the host's `learn` kind (kind-bindings spec
;; §14.2i): the target, the training settings (each control sets its field
;; with set!), the job's progress and its result. `result-panel` takes its
;; values as arguments (capture fixtures pass rows as dicts; the pane passes
;; learn's rows, read the same way: `row.name`), `training-panel` the learn
;; instance (fixtures pass a dict of its fields, read the same way).
(module eseq.patch-learn)
(import eseq.browser)
(import eseq.seq-layout)
(import eseq.seq-step-tabs)
(import eseq.kinds :refer (learn learn-method-options learn-refine-mode-options))

(export cma-config
        training-panel
        result-panel
        panel)

(def close ()
  (if (= eseq.seq-step-tabs/seq-patcher-buffer "")
    (status "No instrument patcher buffer is active")
    (do
      (host-command "close-learn-patch" (dict))
      (eseq.seq-layout/apply-instrument-patcher-layout eseq.seq-step-tabs/seq-patcher-buffer))))

(def local-method? () (= learn.method "Local fit + basin check"))
(def evolution-only? () (= learn.method "Evolutionary search only"))

(def param-status-color (status)
  (match status
    "learnable" :green
    "frozen" :dim
    _ :red))

(def target-picker ()
  (eseq.browser/sample-browser-widget true learn.target-path learn.target-name))

(def plan-row (param)
  (v-stack :key (str "learn-plan-" param.name) :width :fill :gap 0.05
    (h-stack :width :fill :gap 0.4 :align :baseline
      (label param.name :font-size 9.5 :color :white :bg :transparent)
      (box :width 0 :flex 1)
      (label param.status
        :font-size 7.5 :color (param-status-color param.status) :bg :transparent))
    (if (= param.reason "")
      (box :height 0)
      (label param.reason :font-size 7.5 :color :dim :bg :transparent))))

(def plan-list ()
  (box :key "learn-plan-list" :width :fill :height 8.5 :background-color :bg :corner-radius 7 :padding 0.35
    (scroll :width :fill :height :fill
      (v-stack :width :fill :gap 0.3
        (each learn.plan-params |param| (plan-row param))))))

;; A number setting: its label, value, range and decimals, the picker's
;; key, and what a change sets (a learn field, by set!).
(def number-setting (label-text value lo hi decimals key-name set-value)
  (v-stack :width :fill :gap 0.15
    (label label-text :font-size 8 :color :dim :bg :transparent)
    (number-picker :key key-name :width :fill :height 1.25
      :value value :min lo :max hi :decimals decimals
      :on-change set-value)))

;; A choice setting, as number-setting with its options.
(def choice-setting (label-text value options key-name set-value)
  (v-stack :width :fill :gap 0.15
    (label label-text :font-size 8 :color :dim :bg :transparent)
    (dropdown :key key-name :width :fill :height 1.25
      :value value :options options
      :on-change set-value)))

(def evaluation-estimate ()
  (let ((auto (= learn.cma-population 0)))
    (str (if auto "Estimated up to " "Up to ")
      (* learn.cma-generations (if auto 32 learn.cma-population))
      " candidate evaluations"
      (if auto " (trainer resolves auto population)" ""))))

;; The population is auto (0) or at least 4: a step into 1–3 jumps to 4 going
;; up and back to auto going down (set-learn rejects 1–3).
(def population-step (v)
  (if (and (> v 0) (< v 4))
    (if (> v learn.cma-population) 4 0)
    v))

(def cma-config ()
  (v-stack :width :fill :gap 0.45
    (number-setting "GENERATIONS" learn.cma-generations 1 1000 0 "learn-cma-generations"
      (lambda (v) (set! learn.cma-generations v)))
    (number-setting "CANDIDATES / GENERATION (0 = AUTO)" learn.cma-population 0 4096 0
      "learn-cma-population" (lambda (v) (set! learn.cma-population (population-step v))))
    (label (evaluation-estimate) :font-size 7.5 :color :dim :bg :transparent)
    (number-setting "INITIAL SPREAD (SIGMA)" learn.cma-sigma 0.01 10 3 "learn-cma-sigma"
      (lambda (v) (set! learn.cma-sigma v)))
    (number-setting "RANDOM SEED" learn.cma-seed 0 4294967295 0 "learn-cma-seed"
      (lambda (v) (set! learn.cma-seed v)))
    (number-setting "FORWARD BATCH (0 = AUTO)" learn.cma-forward-batch 0 4096 0
      "learn-cma-forward-batch" (lambda (v) (set! learn.cma-forward-batch v)))
    (if (evolution-only?)
      (box :height 0)
      (v-stack :width :fill :gap 0.45
        (number-setting "LOCAL FALLBACK EPOCHS (0 = OFF)" learn.local-epochs 0 2000 0
          "learn-local-epochs" (lambda (v) (set! learn.local-epochs v)))
        (number-setting "SHORTLIST CANDIDATES (0 = BEST ONLY)" learn.cma-continue 0 4096 0
          "learn-cma-continue" (lambda (v) (set! learn.cma-continue v)))
        (number-setting "SHORTLIST ADAM EPOCHS (0 = OFF)" learn.cma-refine-epochs 0 2000 0
          "learn-cma-refine-epochs" (lambda (v) (set! learn.cma-refine-epochs v)))
        (choice-setting "SHORTLIST EXECUTION" learn.cma-refine-mode learn-refine-mode-options
          "learn-cma-refine-mode" (lambda (v) (set! learn.cma-refine-mode v)))
        (number-setting "WINNER TRAINING EPOCHS (0 = OFF)" learn.cma-final-epochs 0 2000 0
          "learn-cma-final-epochs" (lambda (v) (set! learn.cma-final-epochs v)))))))

(def start-label ()
  (if (local-method?) "Start local training"
    (if (evolution-only?) "Start evolutionary search"
      "Search, polish, and train winner")))

(def start-payload ()
  (dict
    :method learn.method
    :epochs learn.epochs
    :cma-generations learn.cma-generations
    :cma-population learn.cma-population
    :cma-sigma learn.cma-sigma
    :cma-seed learn.cma-seed
    :cma-forward-batch learn.cma-forward-batch
    :local-epochs learn.local-epochs
    :cma-continue learn.cma-continue
    :cma-refine-epochs learn.cma-refine-epochs
    :cma-refine-mode learn.cma-refine-mode
    :cma-final-epochs learn.cma-final-epochs
    :pitch-hz learn.pitch-hz
    :gate-frames learn.gate-frames))

;; The target's pitch and note length (keys differ per pane).
(def pitch-setting (key-name)
  (number-setting "PITCH (HZ)" learn.pitch-hz 10 20000 2 key-name
    (lambda (v) (set! learn.pitch-hz v))))

(def gate-setting (key-name)
  (number-setting "GATE (FRAMES)" learn.gate-frames 1 192000 0 key-name
    (lambda (v) (set! learn.gate-frames v))))

(def configure-panel ()
  (v-stack :key "learn-configure" :width :fill :height :fill :gap 0.45
    (target-picker)
    (plan-list)
    (box :width :fill :height 0 :flex 1
      (scroll :width :fill :height :fill
        (v-stack :width :fill :gap 0.45
          (choice-setting "TRAINING METHOD" learn.method learn-method-options "learn-method"
            (lambda (v) (set! learn.method v)))
          (if (local-method?)
            (number-setting "ADAM EPOCHS" learn.epochs 1 2000 0 "learn-epochs"
              (lambda (v) (set! learn.epochs v)))
            (cma-config))
          (pitch-setting "learn-pitch")
          (gate-setting "learn-gate"))))
    (button (start-label) :key "learn-start" :variant :primary :width :fill :height 1.45
      :on-click |x y r| (host-command "start-learn-job" (start-payload))
      :color :white)))

(def step-glyph (step)
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

(def change-color (change) (if (< change 0) :blue :green))

(def epoch-param-row (param)
  (v-stack :key (str "learn-live-param-" param.name) :width :fill :gap 0.08
    (h-stack :width :fill :gap 0.3 :align :center
      (label param.name :font-size 8.5 :width 8 :color :white :bg :transparent)
      (step-glyph param.step)
      (box :width 0 :flex 1)
      (label (fmt "{:+.4}" param.change) :font-size 8.5
        :color (change-color param.change) :bg :transparent))
    (label (fmt "{:.4} → {:.4}" param.from param.value)
      :font-size 7.5 :color :dim :bg :transparent)))

(def loss-graph (losses total-epochs)
  (box :key "learn-loss-curve" :width :fill :height 3.0 :background-color :bg :corner-radius 7 :padding 0.25
    (linegraph
      :width :fill :height :fill
      :values losses
      :total-points total-epochs
      :min 0
      :y-axis true
      :line-color :blue
      :area true)))

(def starting? (stage) (or (= stage "") (= stage "starting")))

(def stage-label (stage)
  (if (starting? stage) "STARTING"
    (match stage
      "cma-es" "EVOLUTIONARY SEARCH"
      "cma-refine-batched" "BATCHED SHORTLIST POLISH"
      "cma-final" "WINNER TRAINING"
      "basin-check" "BASIN CHECK"
      "train" (if (local-method?) "LOCAL TRAINING" "LOCAL FALLBACK")
      _ "SHORTLIST POLISH")))

(def stage-has-epochs (stage)
  (not (or (starting? stage) (= stage "cma-es") (= stage "cma-refine-batched"))))

(def optimization-loss-row (stage index loss)
  (h-stack :key (str "learn-optimization-loss-" index) :width :fill :align :baseline
    (label (if (= stage "cma-es") (str "Candidate " (+ index 1)) "Mean batch loss")
      :font-size 8 :color :dim :bg :transparent)
    (box :width 0 :flex 1)
    (label (fmt "{:.6}" loss) :font-size 8.5 :color :white :bg :transparent)))

(def non-epoch-stage-progress (stage current total optimization-losses losses)
  (box :key "learn-stage-progress" :width :fill :height 6.5
    :background-color :bg :corner-radius 7 :padding 0.55
    (v-stack :width :fill :height :fill :gap 0.35
      (label (if (starting? stage) "Preparing training job…"
          (if (= stage "cma-es")
            (str "Generation " current " / " total)
            (str "Epoch " current " / " total)))
        :font-size 10 :color :white :bg :transparent)
      (if (empty? optimization-losses)
        (label "Waiting for the first optimizer update…"
          :font-size 7.5 :color :dim :bg :transparent)
        (v-stack :width :fill :gap 0.15
          (each (range 0 (len optimization-losses)) |i|
            (optimization-loss-row stage i (nth optimization-losses i)))))
      (if (and (= stage "cma-refine-batched") (> (len losses) 1))
        (loss-graph losses total)
        (box :height 0)))))

;; The training pane of `l`, the learn instance (a fixture passes a dict with
;; its fields). The readouts an epoch moves (the progress with its loss
;; graph, the params) are subtrees that read l themselves, so an epoch
;; re-renders those two and never the target picker.
(def training-progress (l)
  (let ((stage l.stage))
    (if (stage-has-epochs stage)
      (v-stack :width :fill :gap 0.5
        (h-stack :width :fill :align :baseline
          (label (str "Epoch " l.current-epoch " / " l.total-epochs)
            :font-size 10 :color :white :bg :transparent)
          (box :width 0 :flex 1)
          (label (fmt "loss {:.6}" l.loss) :font-size 9 :color :dim :bg :transparent))
        (loss-graph l.losses l.total-epochs))
      (non-epoch-stage-progress stage l.current-epoch l.total-epochs
        l.optimization-losses l.losses))))

(def training-panel (l)
  (v-stack :key "learn-training" :width :fill :height :fill :gap 0.5
    (eseq.browser/sample-browser-widget true l.target-path l.target-name)
    (label (stage-label l.stage) :font-size 7.5 :color :blue :bg :transparent)
    (subtree :key "learn-training-progress" (training-progress l))
    (box :width :fill :height 0 :flex 1 :background-color :bg :corner-radius 7 :padding 0.35
      (scroll :width :fill :height :fill
        (subtree :key "learn-training-params"
          (v-stack :width :fill :gap 0.3
            (each l.epoch-params |param| (epoch-param-row param))))))
    (button "Stop" :key "learn-stop" :variant :secondary :width :fill :height 1.4
      :on-click |x y r| (host-command "stop-learn-job" (dict)) :color :white)))

(def delta-row (delta)
  (h-stack :key (str "learn-delta-" delta.name) :width :fill :gap 0.3 :align :baseline
    (label delta.name :font-size 9 :width 8 :color :white :bg :transparent)
    (label (fmt "{:.4} → {:.4}" delta.from delta.to)
      :font-size 8.5 :color :dim :bg :transparent)
    (box :width 0 :flex 1)
    (label (str (if (< delta.change 0) "−" "+") (fmt "{:.4}" (abs delta.change))) :font-size 8.5
      :color (change-color delta.change) :bg :transparent)))

(def preview-button (text path)
  (button text :variant :secondary :flex 1 :height 1.35
    :on-click |x y r| (host-command "preview-sample" (dict :path path)) :color :white))

(def result-panel (target-path target-name improvement-pct abs-distance basin-check deltas seeded-wav final-wav applied)
  (let ((wrong (= basin-check "wrong_neighborhood")))
    (v-stack :key "learn-result" :width :fill :height :fill :gap 0.5
      (eseq.browser/sample-browser-widget true target-path target-name)
      (label (if wrong
          "Wrong neighborhood — seeded deltas are not trustworthy"
          (fmt "Improved {:.1}%" improvement-pct))
        :font-size 11 :color (if wrong :red :green) :bg :transparent)
      (label (fmt "distance {:.6}" abs-distance) :font-size 8.5 :color :dim :bg :transparent)
      (label "PARAMETER TRAVEL" :font-size 7.5 :color :dim :bg :transparent)
      (box :key "learn-result-list" :width :fill :height 15 :background-color :bg :corner-radius 7 :padding 0.35
        (scroll :width :fill :height :fill
          (v-stack :width :fill :gap 0.35
            (each deltas |delta| (delta-row delta)))))
      (h-stack :width :fill :gap 0.35
        (preview-button "Target" target-path)
        (preview-button "Seeded" seeded-wav)
        (preview-button "Learned" final-wav))
      (label "The live instrument is previewing these learned values. Back or close restores the seed."
        :width :fill :height 1.7 :wrap true :font-size 7 :color :dim :bg :transparent)
      (if applied
        (button "Applied — undo available" :variant :secondary :width :fill :height 1.35
          :on-click |x y r| nil :color :green)
        (button "Apply to instrument" :variant :primary :width :fill :height 1.35
          :on-click |x y r| (host-command "apply-learn-result" (dict)) :color :white))
      (button "Back to configure" :variant :ghost :width :fill :height 1.2
        :on-click |x y r| (host-command "replan-learn-job" (dict)) :color :dim))))

(def error-panel ()
  (v-stack :key "learn-error" :width :fill :height :fill :gap 0.6
    (target-picker)
    (label "Learning stopped" :font-size 11 :color :red :bg :transparent)
    (label learn.error :font-size 9 :color :white :bg :transparent)
    (label "If pitch detection failed, enter the target pitch before retrying."
      :font-size 7.5 :color :dim :bg :transparent)
    (pitch-setting "learn-error-pitch")
    (gate-setting "learn-error-gate")
    (button "Re-run plan" :variant :secondary :width :fill :height 1.35
      :on-click |x y r|
      (host-command "replan-learn-job"
        (dict :pitch-hz learn.pitch-hz :gate-frames learn.gate-frames))
      :color :white)))

(def body ()
  (match learn.phase
    "pick" (target-picker)
    "planning"
    (v-stack :width :fill :gap 0.5
      (target-picker)
      (label "Analyzing learnable parameters…" :font-size 10 :color :dim :bg :transparent))
    "configure" (configure-panel)
    "training" (training-panel learn)
    "result"
    (result-panel
      learn.target-path learn.target-name
      learn.improvement-pct learn.abs-distance
      learn.basin-check learn.result-deltas
      learn.seeded-wav learn.final-wav
      learn.applied)
    _ (error-panel)))

(def panel ()
  (box :key "patch-learn-pane" :width :fill :height :fill :background-color :buffer-bg
    :padding 0.55
    (v-stack :width :fill :height :fill :gap 0.45
      (h-stack :width :fill :align :baseline
        (label "PATCH LEARN" :font-size 11 :color :white :bg :transparent)
        (box :width 0 :flex 1)
        (button "×" :variant :ghost :width 2.2 :height 1.1
          :on-click |x y r| (close) :color :dim))
      (box :key "patch-learn-body" :width :fill :height 0 :flex 1
        (body)))))
