;; Dropdown widget demo

(load "mac-osx-dark.lisp")

(defwidget panel-bg
  :width 1 :height 1
  :shader (sdf/layer
            (sdf/fill (sdf/rounded-rect (* width 1) (* height 1) 0.08)
              (material :color (rgba 0.16 0.16 0.17 1)))))

;; ── State ──

(def timebase (state "8n"))
(def waveform (state "sine"))
(def sync-mode (state "free"))

;; ── UI ──

(effect
  (box :padding 2
    (box :background "panel-bg"
      (v-stack :padding 1.5 :gap 1.5

        (label "Dropdown Demo" :color :white :bg :transparent :font-size 16)

        (h-stack :gap 1 :align :center
          (label "timebase:" :color :gray :bg :transparent :font-size 13 :width 8)
          (dropdown :value timebase
            :options '("1n" "2n" "4n" "8n" "16n" "32n" "8t" "16t")
            :on-change (lambda (v) (set! timebase v))
            :width 8 :height 1.5 :font-size 13))

        (h-stack :gap 1 :align :center
          (label "waveform:" :color :gray :bg :transparent :font-size 13 :width 8)
          (dropdown :value waveform
            :options '("sine" "triangle" "sawtooth" "square" "noise")
            :on-change (lambda (v) (set! waveform v))
            :width 10 :height 1.5 :font-size 13))

        (h-stack :gap 1 :align :center
          (label "sync:" :color :gray :bg :transparent :font-size 13 :width 8)
          (dropdown :value sync-mode
            :options '("free" "tempo" "transport")
            :on-change (lambda (v) (set! sync-mode v))
            :width 10 :height 1.5 :font-size 13))

        (label (str "timebase=" timebase "  waveform=" waveform "  sync=" sync-mode)
          :color :gray :bg :transparent :font-size 12)))))
