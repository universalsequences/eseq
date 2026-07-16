;; Tree widget test — macOS-style collapsible folder hierarchy

;; Load macOS dark theme
(load "../sequencer/ui/themes/mac-osx-dark.lisp")

(defwidget panel-bg
  :width 1 :height 1
  :shader (sdf/layer
            (sdf/fill (sdf/rounded-rect (* width 1) (* height 1) 0.02)
              (material :color (rgba 0.16 0.16 0.17 1)))))

(def selected-item (state ""))

(effect (v-stack :padding 1 :gap 1
    (label (str "Selected: " selected-item) :color :white :bg :transparent :font-size 14)

    (box :background "panel-bg" :padding 0 :flex 1
      (scroll :flex 1
        (tree
          ;; macOS Finder-style alternating rows
          :row-bg-even  '(0.16 0.16 0.17)
          :row-bg-odd   '(0.19 0.19 0.20)
          :selected-bg  '(0.00 0.35 0.82)
          :folder-color '(0.88 0.88 0.89)
          :file-color   '(0.62 0.62 0.65)
          :chevron-color '(0.50 0.50 0.53)
          :items '(
            (:label "drums" :children (
                (:label "acoustic" :children (
                    (:label "kick_tight.wav")
                    (:label "kick_room.wav")
                    (:label "snare_crack.wav")
                    (:label "snare_buzz.wav")))
                (:label "electronic" :children (
                    (:label "808_kick.wav")
                    (:label "808_snare.wav")
                    (:label "909_hat_closed.wav")
                    (:label "909_hat_open.wav")))
                (:label "percussion" :children (
                    (:label "conga_high.wav")
                    (:label "conga_low.wav")
                    (:label "shaker.wav")
                    (:label "tambourine.wav")))))
            (:label "synths" :children (
                (:label "pads" :children (
                    (:label "warm_pad.wav")
                    (:label "glass_pad.wav")
                    (:label "string_pad.wav")))
                (:label "leads" :children (
                    (:label "saw_lead.wav")
                    (:label "square_lead.wav")))
                (:label "bass" :children (
                    (:label "sub_bass.wav")
                    (:label "reese_bass.wav")
                    (:label "acid_bass.wav")))))
            (:label "fx" :children (
                (:label "risers" :children (
                    (:label "white_noise_rise.wav")
                    (:label "tonal_rise.wav")))
                (:label "impacts" :children (
                    (:label "boom.wav")
                    (:label "crash_cymbal.wav")))
                (:label "textures" :children (
                    (:label "vinyl_crackle.wav")
                    (:label "tape_hiss.wav")))))
            (:label "vocals" :children (
                (:label "chops" :children (
                    (:label "hey.wav")
                    (:label "yeah.wav")
                    (:label "oh.wav")))
                (:label "adlibs" :children (
                    (:label "skrrt.wav")
                    (:label "woah.wav"))))))
          :on-select (lambda (item) (set! selected-item (get item :label)))
          :on-activate (lambda (item) (status (str "Activate: " (get item :label)))))))))
